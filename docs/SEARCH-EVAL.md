# Measuring search quality

`crates/penguin-eval` checks whether a change to search makes it better or worse. It has five parts:

- A synthetic mailbox built on your machine.
- A labelled set of 570 queries with graded relevance judgments: 200 in nine core categories and 370 edge cases in ten more ([SEARCH-CASES.md](SEARCH-CASES.md)).
- A harness that runs the query set through Penguin's search and reports nDCG@10, MRR, Recall@10/50 and latency for each query type.
- A before/after diff with significance tests.
- A mode that runs the same metrics on your own mailbox, read-only, with judgments that stay on your Mac.

The app doesn't use this crate, and it doesn't ship.

```sh
# Build, then run the keyword baseline (a few seconds to build the corpus, a few to run)
cargo run -p penguin-eval --release -- run
cargo run -p penguin-eval --release -- run --mode hybrid --embedder hash   # vectors + RRF
cargo run -p penguin-eval --release -- diff crates/penguin-eval/baselines/keyword.json target/search-eval/hybrid-hash.json
cargo run -p penguin-eval --release -- check           # corpus stats; validates every query
cargo run -p penguin-eval --release -- show pa-water-heater   # one query: its judgments and ranking
```

On the shared Linux box, run cargo through `scripts/box-cargo.sh` (for example `scripts/box-cargo.sh run -p penguin-eval --release -- run`).

## How to use it for a ranking change

1. Run `search-eval run` on `main` and on your branch, on the same day and with the same build of penguin-eval. The results go to `target/search-eval/<name>.json`, or wherever `--out` points.
2. Run `search-eval diff before.json after.json`. For each query category it gives:
   - the change in each metric;
   - a paired randomization-test p-value;
   - wins and losses;
   - the change in median latency.

   Below that it lists the queries that moved most in each direction.
3. A change is worth keeping when:
   - it improves the categories it was aimed at;
   - it does not significantly hurt identifier, operator or name queries, where keyword search is already near 1.0 (the research doc's rule: "exact matches and explicit filters authoritative");
   - it stays within the latency budget in `docs/PERFORMANCE.md`.

The committed baseline in `crates/penguin-eval/baselines/` is a reference point. For a real comparison, run both sides fresh (see [Determinism](#determinism)).

## What is measured

### The unit is the conversation

Penguin's result list shows conversations, so relevance is judged per conversation. The document id is `account/thread`. Each query retrieves 100 conversations.

### Metrics

The metrics match `trec_eval`, the evaluation tool used by NIST TREC and, through pytrec_eval, by BEIR. `run --trec DIR` writes `qrels.txt` and `run.txt` so you can check the numbers independently:

```sh
trec_eval -c -l 2 -m ndcg_cut.10 -m recip_rank qrels.txt run.txt
```

We checked our output against a separate implementation of the trec_eval formulas: all queries match exactly (checked on the original 200).

| Metric | Definition | Why |
|---|---|---|
| **nDCG@10** | Gain is the grade itself (linear). Rank *i* is discounted by log2(*i*+1). The ideal ranking uses every judged conversation for the query, retrieved or not. This is `trec_eval ndcg_cut.10`, based on Järvelin & Kekäläinen's cumulated gain. | It is the one metric that uses graded judgments and focuses on the first screen. BEIR uses it as its primary metric because it suits both binary and graded tasks. The TREC Deep Learning track uses it because it "makes use of our 4-level judgments and focuses on the first results that users will see". |
| **MRR** | Mean over queries of 1 / (rank of the first relevant conversation), or 0 if none is retrieved. Defined in the TREC-8 QA track. | Most email searches are for a known item (Ai et al. 2017; Elsweiler & Ruthven 2007). What matters is how far down the one you want is. The TREC 2005 Enterprise track scored its email known-item task with MRR, and so did Kim & Croft's pseudo-desktop study. |
| **Recall@10, @50** | Relevant conversations in the top k, divided by min(k, R), where R is the number of relevant conversations ("capped" recall). | Recall@10 asks whether it is on the first screen. Recall@50 asks whether it is anywhere the list reaches. Capping lets a query with 300 relevant conversations still reach 1.0. |
| **empty** | Queries with no results at all. | Keyword search's worst failure: showing nothing. |
| **p50 / p95 ms** | Nearest-rank percentiles over 5 timed repetitions of every query, after one warm-up run. | The latency cost of each ranking change, in the same run. |

**Relevant** for MRR and recall means grade ≥ 2, the same as `trec_eval -l 2`. Grade 1 ("related") counts toward nDCG but is not treated as a hit.

**Averages.** Each metric is the unweighted mean over the queries in a category. The **all** row is the mean over all 570 queries. The category mix is designed, not sampled from real traffic. So read decisions per category, and treat **all** as a summary only.

### Graded relevance

The scale is the four levels of the TREC 2019 Deep Learning track (Craswell et al.), in its passage-task wording. An email either is the thing you wanted or contains the answer, which is what the passage wording describes.

| Grade | Meaning here | TREC DL (passage) |
|---:|---|---|
| 3 | The conversation you were looking for, or the one that answers the question. | "Perfectly relevant: the passage is dedicated to the query and contains the exact answer" |
| 2 | Squarely about the need: one of several equally good items (every electricity bill for "electricity bill"), or a partial answer. | "Highly relevant: … has some answer … but the answer may be a bit unclear" |
| 1 | On topic but does not satisfy the need: past Lisbon trips for "when does my flight to Lisbon leave", other Crestline invoices for "Crestline invoice past due". | "Related: seems related to the query but does not answer it" |
| 0 | Not relevant. | "Irrelevant" |

One consequence: when a known-item query has "related" conversations, a ranking that shows only the target scores below 1.0 on nDCG@10. For example, `crestline invoice past due` scores 0.46. The ideal ranking would show the target first and the related invoices after it. MRR and Recall@10 still read 1.0 for that query.

## The synthetic mailbox

Real personal mail can't be the shared test set. Wang, Bendersky, Metzler & Najork (Google, SIGIR 2016) explain why for personal search: each user sees only their own corpus, and "TREC-like document relevance judgments by third party raters … are difficult to obtain due to privacy restrictions". Research on desktop and email search therefore builds simulated collections:

- Kim & Croft (CIKM 2009) built pseudo-desktop collections around people in the TREC Enterprise email and generated known-item queries.
- Azzopardi, de Rijke & Balog (SIGIR 2007) built simulated known-item queries and validated them against real ones.

Penguin's corpus follows that approach. Real-mail mode (below) is the check against reality.

`corpus/` generates the same mailbox from a seed (`--seed`, default 42) in about a second. It loads the mailbox into a real `penguin_core::Store` through the public API (`upsert_account`, `replace_labels`, `upsert_messages`, `optimize`), exactly as sync does. That means the FTS index, quoted-text splitting, people table and ranking features are the production ones.

| | |
|---|---|
| Messages / conversations | 19,229 / 6,477 (19,183 / 6,431 before the edge-case mail) |
| Accounts | Work, `alex.moreno@northwind.example` (11,709). Personal, `alexmoreno@mailbox.example` (5,546). Freelance studio, `hello@morenostudio.example` (1,928). |
| Work threads | 1,466 conversations, 10,683 messages. 2–13 messages each, with replies quoting the previous message under an `On … wrote:` attribution, 1–3 colleagues each, 13 topics (budget, launch, hiring, incidents, design, security, customers, vendors…). |
| Personal and family | 638 conversations: friends, plans, photos, and family in Valencia writing in Spanish. Across the mailbox, 373 conversations are in Spanish. |
| Freelance clients | 342 conversations, some in Spanish. Also 122 invoices, sent and received, with PDFs. |
| Newsletters and promotions | 1,002 newsletter issues from seven newsletters (one of them in Spanish) and 420 promotions, with HTML bodies and `List-Unsubscribe`. |
| Receipts | 624. Rides, food delivery, coffee, online orders, apps, a Spanish shop. Across receipts and flights, 275 conversations carry schema.org JSON-LD (`Order`, `FlightReservation`). About half of the JSON-LD receipts are HTML-only, so their text is the HTML's text. |
| Shipping | 123 threads (249 messages): shipped, out for delivery, delivered. Each links to an order number and has a tracking number. |
| Bills | 174 monthly statements from utilities, internet, phone, streaming and insurance, with statement PDFs. |
| Travel | 96 flight bookings (with check-in reminders) and 62 hotel bookings across 12 cities, with booking codes. |
| Calendar | 470 invitations with `.ics` attachments, some with an "Accepted" reply. |
| Codes | 462 verification-code emails from 7 senders, one of them in Spanish. |
| Social | 430 notifications that name friends. |
| Attachments | 1,740 messages: PDFs, spreadsheets, documents, images, a zip. |
| Hand-written targets | 51 conversations (`corpus/planted.rs`) that the paraphrase, natural-language, question and Spanish queries point at. |
| Edge-case targets | 46 conversations (`corpus/edge.rs`) for the edge-case categories. They cover: <br>• diacritics (José Núñez), letters that don't decompose (Søren Ødegård, Straße), CJK (田中 美咲), O'Brien, Anne-Marie <br>• a sender who signs "Bob", plus-addressing, an internationalized domain, subdomains, emoji, an ampersand <br>• tracking numbers printed in groups or as one run, European amounts, phone numbers with country codes, URLs, versions <br>• British spellings, Trash and Spam <br>• oddly named, big and mis-typed attachments (HEIC, a PDF sent as octet-stream, CSV) <br>• mail either side of local midnight, and newer look-alikes that make each date filter do the work |
| User labels | Receipts, "Tax Docs 2025" (spaces), Clients/Fernhill Bakery and two other nested client labels, Travel/Lisbon 2026, Work/Atlas/Finance (three levels), Family, Follow-up (a hyphen), and the parent labels Gmail creates. They are filed on background mail the way a person would file it (`Corpus::label_threads`). |
| Categories and state | Received mail with no other category is Primary (`CATEGORY_PERSONAL`), as in Gmail. The facts record inbox, unread, starred, Trash, Spam, labels, categories, the recipients of sent mail and attachment-size thresholds, so filters can be judged exactly. |

### Design choices

- **Fictional throughout.** Every person is invented and every domain ends in `.example` (RFC 2606); a test enforces it. No real company or brand names appear.
- **Deterministic.** Each mail family has its own random stream (`Rng::fork`), so resizing one family leaves the others unchanged. Dates are set relative to the day the corpus is generated, which keeps every message's age identical from run to run.
- **Volume grows toward the present, and a few correspondents dominate.** Dates are skewed toward the present and correspondents are Zipf-distributed, as in a real mailbox. Recency and "people you write to" boosts behave as they would on real mail.
- **Paraphrase variety.** Background text comes from phrase banks with several wordings per idea, so two messages on the same topic rarely share every word.
- **Deliberate vocabulary mismatch in the targets.** Some examples:
  - The plumber's email says "swap out the boiler tank" and "the hot tap will be off", never "water heater".
  - The Lisbon booking never says "Portugal" or "plane tickets".
  - The electricity company's statement never says "electricity".
  - The vet's reminder says "Biscuit" and "rabies booster", never "dog" or "shots".

  These are the targets that search by meaning has to find and keyword search cannot.
- **Realistic distractors.** There are other Lisbon trips, two Mikes, 30 other invoices from the same vendor, other 6-digit codes, other bank emails, a paella recipe with the same subject from a different relative, and "migration" in unrelated work threads.

## The query set

The 200 core queries (`src/queries.rs`) each have exactly one category and optional facets. Their categories come from what the literature says people do in email search:

- known-item re-finding dominates (Ai, Dumais, Craswell & Liebling, WWW 2017; Elsweiler & Ruthven, SIGIR 2007);
- email search mixes time and relevance (Carmel et al., CIKM 2015);
- email known-item search was a TREC Enterprise track task (Craswell, de Vries & Soboroff, TREC 2005).

They also come from the known gaps in email search: meaning, questions, dates in words, and Spanish.

| Category | n | What it tests | Example |
|---|---:|---|---|
| operator | 26 | Operators, mostly combined with words. | `from:theo "past due"`, `has:invite design review`, `(from:marco OR from:julia) invoice` |
| name | 17 | A person's or company's name or address. | `priya`, `mike` (two Mikes), `linden partners` |
| identifier | 23 | Exact codes and numbers. | `K7QX2M`, `DC-55120`, `SW4829105533`, `$649.00`, `(415) 555-0138` |
| natural | 24 | A natural description that shares words with the target. | `the pdf mike sent about the lease`, `pager rotation november` |
| paraphrase | 33 | Vocabulary mismatch: the target doesn't use the query's words. | `water heater replacement`, `plane tickets to portugal`, `electricity bill` |
| question | 28 | A question, as typed into Ask. | `when does my flight to lisbon leave?`, `how much is the new rent?` |
| misspelling | 17 | Typos and sound-alikes. | `lisbn flight`, `recipt deskcraft`, `fishing emails` |
| spanish | 18 | Spanish queries on Spanish mail, and queries across English and Spanish. | `receta paella abuela`, `grandma's paella recipe`, `factura de la luz` |
| time | 14 | Time windows, in words or with `date:`. | `dentist last spring`, `from:swiftcab date:"last month"`, `electricity bill from march` |

The ten edge-case categories (`src/edge_queries.rs`, 370 queries) come from a survey of what people type into Gmail, Outlook, Apple Mail, Fastmail, Proton, notmuch, mu, aerc, Thunderbird, himalaya and meli, and from where it fails. The taxonomy, the examples and the sources are in [SEARCH-CASES.md](SEARCH-CASES.md).

| Category | n | What it tests | Example |
|---|---:|---|---|
| syntax | 36 | OR precedence, groups, Gmail braces, `NOT`, `AROUND`, `*`, quotes, malformed input | `{from:marco from:julia} invoice`, `invoice NOT crestline`, `(lease renewal` |
| place | 44 | Labels with spaces and nesting, folders, states, categories, accounts, Trash and Spam | `label:tax docs 2025`, `label:atlas/finance`, `in:bin comeback` |
| attachment | 33 | `has:` kinds, file names, sizes, other products' operators | `filename:"q3 report"`, `size:30000000`, `hasattachment:yes lease` |
| date | 37 | Date formats, local midnight, relative dates, numbers in words, weekends, quarters | `on:18.08.2026`, `kestrel after:<unix seconds>`, `compost date:"two weeks ago"` |
| people | 47 | Diacritics, ø/ß, CJK, apostrophes, hyphens, nicknames, plus-addressing, subdomains, emoji | `soren odegard`, `o'brien`, `rob kowalski`, `to:me lumen lamps` |
| format | 40 | Identifiers typed another way | `1Z4F8A620311872290`, `INV20417`, `2450`, `+1 415 555 0138`, `2.14.3` |
| morphology | 38 | Plurals, inflections, British/American, compounds, possessives, CJK inside a run | `crestline invoices`, `color proofs`, `oncall handoff`, `会議室` |
| messy | 35 | Short, long and pasted queries, subjects with Re:/Fwd:, punctuation, typos | `RE: FW: Past due: invoice INV-20417`, `LeaseRenewal`, `crestlnie invoice` |
| mixed | 31 | Operators with plain English, and conflicting filters (expect nothing) | `from:kwame money approved for the project`, `is:unread is:read` |
| recall | 29 | Descriptions, questions and other languages over the edge-case mail | `rent going up`, `what's the tracking number for the lamp?`, `réservation hôtel madrid` |

Facets cut across the categories:

- `known-item` (325 queries): exactly one target.
- `broad` (120): many equally good results.
- `cross-lingual` (16).
- `question` (47).
- `expect-empty` (18): the right answer is no result at all (conflicting or impossible filters, a phrase nothing contains). Such a query scores 1 on every metric when nothing comes back and 0 otherwise, and has no judgment rules. It checks that search by meaning doesn't leak past a filter that nothing passes.
- `operator` (4): time queries that use `date:`.
- `recency` (3): "the latest code".

Queries in the `date` category spell dates out for the day the corpus is generated (`on:2026-08-18`), so `all_queries` takes the corpus's `now`.

### Judgments are rules, and complete for the concept

Each query's judgments are rules over conversation facts (`src/judge.rs`). Every conversation carries machine-readable tags such as:

- `kind:flight`, `city:lisbon`, `merchant:cedarvalley`
- `from:<address>`, `person:<key>`
- `pdf-from:<address>` (the message that carried a PDF)
- `plant:<id>`

A rule reads, for example, "flights whose city is Lisbon → 2", or "the planted booking → 3, other Lisbon flights → 2, Lisbon hotels → 1".

Time rules test the same calendar windows the search parser uses (`src/window.rs` mirrors `penguin-core/src/dates.rs`: meteorological seasons, "last spring" = the most recent completed one). The planted targets are placed inside those windows, so they stay correct whatever day the run happens.

Because the rules are applied to all 6,477 conversations, the judgments cover every conversation that shares the concept, not just the one the query was written about. That avoids the incomplete-judgment problem:

- Buckley & Voorhees (SIGIR 2004) introduced bpref for evaluation with incomplete judgments.
- BEIR measures the unjudged share of the top 10 as Hole@10 and shows that unjudged results can understate a new system.

On the synthetic corpus every result is judged; unmatched means 0.

`search-eval check` catches mistakes in the query set. It fails when:

- a query has nothing relevant to find (or, for `expect-empty`, has judgment rules);
- a rule names a tag that no conversation has (a typo);
- a query id is duplicated.

`cargo test -p penguin-eval` runs the same check, plus determinism and the `.example`-only rule.

### How many queries

Voorhees & Buckley (SIGIR 2002) measured how often a ranking of two systems flips when a different topic set is used, and found error rates "larger than anticipated" for small sets. In the TREC Robust track analysis (Voorhees, 2005), treating differences under 5% as ties, the swap rate was 2.4% at 50 topics and 0.7% at 100. 570 queries overall is comfortable. A single category has 14–47 queries, so treat per-category changes as indicative and rely on the significance test. Add queries to a category before drawing fine conclusions about it.

## Retrievers

`--mode` picks what is evaluated (`src/retrieve.rs`):

- **keyword**: `Store::search`, exactly as the app calls it. When hybrid ranking lands inside `Store::search`, this mode measures it with no change here.
- **ask**: `Store::ask`. The answer's cited messages, in order, are the ranking. After loading, the corpus goes through the structured-fact scanner (`Store::extract_pending`), as the app's background task does after sync, so flight, order, parcel and bill questions are answered from extracted facts; `--no-extract` skips it (Ask then extracts search hits on the fly, as during the app's first backfill). `cargo test -p penguin-eval --release --test extraction -- --nocapture` scores the extractors against the corpus's own tags (docs/ASK.md).
- **vector**: an `Embedder` with a `VectorIndex` (penguin-semantic's contract), searched alone.
  - Each message is split into chunks: chunk 0 is the sender, subject and opening; after that come 120-word windows of the authored text.
  - Quoted history is left out, because it belongs to the message it quotes.
  - A conversation scores its best chunk.
- **hybrid**: a reference fusion of keyword and vector results, using reciprocal rank fusion with k = 60 (Cormack, Clarke & Büttcher, SIGIR 2009: "k = 60 was fixed during a pilot investigation"). Operators stay hard filters: with operators present, only conversations that pass them are fused in from the vector side. Those candidates are capped at the 500 newest that pass; queries with `OR` or parentheses fuse no vector results. This hybrid is a yardstick for the product's own hybrid, not a design for it.
- **penguin**: the product's own hybrid search, `Store::search_hybrid`, over a vector index cut by `penguin_semantic::chunk_message` (what the app indexes). See [SEARCH-RANKING.md](SEARCH-RANKING.md).

Embedders: `hash` (the stand-in), or `file:PATH` for vectors a real model computed offline. `search-eval texts` lists every passage and query text `--mode penguin` embeds (queries after hybrid search's own rewrite), and `crates/penguin-core/examples/ranking/embed.py` embeds them with a fastembed model, reusing vectors it already has. `show QUERY --mode penguin` also prints the rewrite.

**Plugging in the real model.** Implement `penguin_semantic::Embedder` and add it to `retrieve::embedder_by_name`, then run `--mode vector|hybrid --embedder <name>`. The index is built in memory from the generated messages, and the build time is printed. The `hash` embedder is the stand-in bag of hashed words: it knows nothing about meaning and shows the harness works end to end, not what a model can do.

## Before/after diffs

`search-eval diff A.json B.json` pairs the two runs by query id.

**Significance.** For nDCG@10 and MRR in each category it runs a two-sided paired randomization (permutation) test on the per-query scores, with 100,000 permutations. Smucker, Allan & Carterette (CIKM 2007) compared the tests used in IR evaluation. They recommend the randomization test "when it is applicable", using the same statistic as the reported difference. They also found that the t-test, bootstrap and randomization tests largely agree, and that the Wilcoxon and sign tests "should no longer be used". The p-value counts the observed labelling as one of the permutations, so it is never 0.

**Multiple comparisons.** Each diff runs 20 tests, so about one in twenty can pass by chance. Believe a significant change in the category you targeted, and be sceptical of a lone significant change elsewhere.

**Warnings.** The diff warns when:

- the corpora differ (different seed or generator, detected by the structural fingerprint);
- the corpora were generated on different days (calendar windows moved);
- a query's judgments differ between the runs (per-query judgment hash).

**Latency** is measured in the same run as the relevance numbers. Compare it only between runs made back to back on the same machine; it is informational. Speed claims belong to `docs/PERFORMANCE.md` benchmarks.

### Determinism

The ranking is deterministic, and debug builds assert it on every repetition. The same seed, build and day give identical relevance numbers. On a different day the mailbox has the same structure and message ages, but:

- calendar words in bodies change ("Tuesday" → "Wednesday");
- "last month" covers a different set of background receipts.

So a day-apart comparison can move the time category slightly. Run both sides the same day.

## Real-mail mode (your Mac)

`search-eval real` scores Penguin on your own mail: queries you write, judgments you make, with nothing leaving the machine.

```sh
mkdir -p eval-private
$EDITOR eval-private/queries.txt
cargo run -p penguin-eval --release -- real            # judge interactively, then score
cargo run -p penguin-eval --release -- real --no-judge # just score (and list unlabelled queries' top 10)
cargo run -p penguin-eval --release -- real --mode ask --name ask
cargo run -p penguin-eval --release -- diff eval-private/runs/keyword.json eval-private/runs/after.json
```

### The database is opened read-only

It opens the app's database with `Store::open_read_only`, the same path the MCP server uses:

- the connection is opened `SQLITE_OPEN_READ_ONLY` with `PRAGMA query_only`;
- nothing is migrated or written;
- it works while Penguin is running.

The path follows the app's rule (`apps/desktop/src-tauri/src/ops.rs`): `$PENGUIN_DATA_DIR/penguin.db` if that is set, otherwise `~/Library/Application Support/co.gluska.penguin/penguin.db`. Use `--db PATH` to point elsewhere.

### The queries file

The queries file is `eval-private/queries.txt` (or `--queries FILE`), one query per line:

```text
# category | query | optional helper query
known-item | lease pdf from mike
paraphrase | plane tickets to portugal | K7QX2M
question   | when does my flight leave?
spanish    | factura de la luz
```

The optional third column is a helper query that finds the target another way, such as a code or an exact subject. It only puts the target in front of you for judging. Without it, you can't label a paraphrase query whose target keyword search misses.

The failed Spark searches that started this project are the best queries you can write. Research doc §8 suggests 100–200 real tasks, with a target of ≥ 95% success in the first ten.

### Judging

For each query you see the pool of candidates, one per line: date, sender, subject and a snippet, plus your earlier grade if you gave one. The pool is the union of the top 10 from keyword search, from Ask and from the helper query, interleaved. This is TREC-style pooling: the union of several systems' top results gets judged. You type grades for the unjudged ones in order, separated by spaces:

- 3 = the one
- 2 = highly relevant
- 1 = related
- 0 = not relevant

Type `-` to skip one result, press Enter to skip the query, or type `q` to stop. Judgments are saved after every query.

Judging turns on when stdin is a terminal or when you pass `--judge`; `--no-judge` turns it off. Later runs ask only about new, unjudged results. When a ranking change surfaces conversations you haven't seen, you are asked about those alone.

### Scoring

Only queries with at least one relevant judgment are scored, as `trec_eval` skips queries with no relevant documents. Unjudged results count as not relevant, and the report shows how many queries have unjudged results in their top 10. That is the judged@k idea (ir_measures): until you judge those results, the numbers are pessimistic.

Vector and hybrid modes aren't offered here. They need an index over your mailbox, which the app will own. Once hybrid ranking is inside `Store::search`, `keyword` measures it.

### Nothing here is committed

`eval-private/` is gitignored. It holds `queries.txt`, `judgments.json` (grades with the subject and sender, for your review) and `runs/*.json` (query texts and conversation ids). Never commit or share it; `--private DIR` puts it somewhere else.

## Baseline and the edge-case loop

The saved baselines are `crates/penguin-eval/baselines/keyword.json` (`Store::search`) and `ask.json` (`Store::ask`).
- **Corpus:** seed 42, generated 2026-09-27, fingerprint `59ad7d3f2c436232`, 570 queries.
- **Code:** the edge-case branch ending at `269d8de`.
- **Machine:** the shared Linux build box (8 threads, loaded).
- **Numbers:** relevance is deterministic. Latency is noisy; take speed from `docs/PERFORMANCE.md` and `bench_search`.

**Keyword (`Store::search`)**

| Category | n | nDCG@10 | MRR | R@10 | R@50 | Empty | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| identifier | 23 | 1.000 | 1.000 | 1.000 | 1.000 | 0 | 0.6 | 0.9 |
| operator | 26 | 0.959 | 1.000 | 1.000 | 0.983 | 0 | 2.6 | 7.0 |
| name | 17 | 0.981 | 1.000 | 0.971 | 1.000 | 0 | 5.2 | 21.9 |
| natural | 24 | 0.921 | 0.958 | 0.958 | 0.958 | 1 | 1.8 | 4.8 |
| spanish | 18 | 0.346 | 0.444 | 0.355 | 0.284 | 10 | 2.8 | 7.0 |
| time | 14 | 0.286 | 0.286 | 0.286 | 0.286 | 10 | 2.2 | 5.5 |
| question | 28 | 0.136 | 0.112 | 0.143 | 0.143 | 23 | 1.9 | 5.8 |
| misspelling | 17 | 0.765 | 0.765 | 0.765 | 0.765 | 3 | 3.9 | 11.9 |
| paraphrase | 33 | 0.015 | 0.030 | 0.013 | 0.013 | 31 | 2.5 | 5.0 |
| syntax | 36 | 0.998 | 1.000 | 0.997 | 0.979 | 1 | 2.5 | 5.9 |
| place | 44 | 0.994 | 1.000 | 1.000 | 0.999 | 2 | 2.5 | 7.0 |
| attachment | 33 | 1.000 | 1.000 | 1.000 | 1.000 | 0 | 0.5 | 2.8 |
| date | 37 | 0.910 | 0.946 | 0.922 | 0.946 | 3 | 1.1 | 1.6 |
| people | 47 | 0.979 | 0.979 | 0.979 | 0.979 | 1 | 1.0 | 9.1 |
| format | 40 | 1.000 | 1.000 | 1.000 | 1.000 | 0 | 0.9 | 3.7 |
| morphology | 38 | 0.885 | 0.921 | 0.903 | 0.907 | 3 | 1.9 | 4.5 |
| messy | 35 | 0.937 | 0.943 | 0.933 | 0.933 | 2 | 1.7 | 6.0 |
| mixed | 31 | 0.771 | 0.774 | 0.774 | 0.772 | 21 | 1.2 | 2.6 |
| recall | 29 | 0.110 | 0.125 | 0.138 | 0.138 | 24 | 3.4 | 8.6 |
| **all** | 570 | **0.772** | **0.786** | **0.779** | **0.778** | 135 | 1.6 | 6.0 |

### Before and after the edge-case work

Both sides ran on the same day, on the same corpus and with the same penguin-eval build. "Before" is penguin-core at `9c9df2c`, the commit that added the edge-case queries. "After" is the end of the loop. The columns are:
- **keyword:** `Store::search`.
- **hybrid, Gemma:** `--mode penguin --embedder gemma`, the product path with EmbeddingGemma.
- **hybrid, stand-in:** `--mode penguin` with the bag-of-words stand-in (`hash-bow`).
- **Ask:** `Store::ask`.

nDCG@10:

| Category | n | keyword before | keyword after | hybrid Gemma before | hybrid Gemma after | hybrid stand-in before | hybrid stand-in after | Ask before | Ask after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| identifier | 23 | 1.000 | 1.000 | 0.997 | 0.997 | 0.996 | 0.996 | 1.000 | 1.000 |
| operator | 26 | 0.959 | 0.959 | 0.967 | 0.967 | 0.961 | 0.961 | 0.894 | 0.894 |
| name | 17 | 0.981 | 0.981 | 0.950 | **0.964** | 0.995 | 0.995 | 0.880 | 0.880 |
| natural | 24 | 0.921 | 0.921 | 0.987 | 0.987 | 0.966 | 0.973 | 0.969 | 0.969 |
| paraphrase | 33 | 0.000 | 0.015 | 0.827 | 0.826 | 0.032 | 0.040 | 0.376 | 0.388 |
| question | 28 | 0.107 | 0.136 | 0.882 | 0.882 | 0.328 | 0.350 | 0.522 | 0.561 |
| misspelling | 17 | 0.059 | **0.765** | 0.851 | **0.910** | 0.736 | **0.853** | 0.495 | **0.786** |
| spanish | 18 | 0.346 | 0.346 | 0.753 | 0.761 | 0.379 | 0.375 | 0.522 | 0.522 |
| time | 14 | 0.286 | 0.286 | 0.736 | 0.741 | 0.524 | 0.533 | 0.512 | **0.602** |
| syntax | 36 | 0.859 | **0.998** | 0.955 | **0.998** | 0.932 | **0.998** | 0.864 | 0.914 |
| place | 44 | 0.926 | **0.994** | 0.926 | **0.994** | 0.926 | **0.994** | 0.731 | 0.768 |
| attachment | 33 | 0.909 | **1.000** | 0.959 | **1.000** | 0.909 | **1.000** | 0.957 | 0.976 |
| date | 37 | 0.723 | **0.910** | 0.814 | **0.964** | 0.772 | **0.964** | 0.755 | **0.922** |
| people | 47 | 0.809 | **0.979** | 0.855 | **0.979** | 0.834 | **0.971** | 0.878 | **0.973** |
| format | 40 | 0.550 | **1.000** | 0.686 | **1.000** | 0.565 | **1.000** | 0.663 | **1.000** |
| morphology | 38 | 0.136 | **0.885** | 0.857 | **0.987** | 0.550 | **0.849** | 0.644 | **0.896** |
| messy | 35 | 0.559 | **0.937** | 0.929 | **0.984** | 0.834 | **0.925** | 0.836 | **0.987** |
| mixed | 31 | 0.706 | **0.771** | 0.909 | **0.958** | 0.753 | **0.813** | 0.542 | 0.571 |
| recall | 29 | 0.091 | 0.110 | 0.923 | 0.920 | 0.232 | 0.255 | 0.750 | 0.758 |
| **all** | 570 | 0.594 | **0.772** | 0.882 | **0.951** | 0.701 | **0.806** | 0.736 | **0.827** |
| the 200 core queries | 200 | 0.505 | 0.571 | 0.891 | 0.898 | 0.624 | 0.640 | 0.682 | 0.721 |
| the 370 edge cases | 370 | 0.642 | **0.881** | 0.877 | **0.980** | 0.743 | **0.895** | 0.764 | **0.884** |

Bold marks a change of 0.01 or more. Across all 570 queries the paired randomization test gives p < 0.001 in every mode. Queries better / worse: keyword 111 / 0, hybrid Gemma 59 / 4, stand-in 80 / 4, Ask 73 / 2.
- **Queries with no result at all:** keyword 234 → 135, hybrid 30 → 19.
- **The 18 `expect-empty` queries (conflicting filters):** keyword and hybrid return nothing for all of them, before and after. Ask answers 14 of them anyway, before and after, which is an open issue for Ask.

**What moved down, and why:**
- **Hybrid Gemma, paraphrase −0.001 and recall −0.003.** Two queries: `hotel in lisbon` (0.88 → 0.85) and `réservation hôtel madrid` (0.91 → 0.82). Keyword search now finds the "Staywell Hotels" bookings through the word form `hotels`. They are grade-2 answers (other hotels in the same city), and they now rank around the grade-3 guesthouse. RR stays 1.0 for the Madrid query. The Lisbon target moves from rank 1 to rank 4, behind three relevant bookings.
- **Hybrid Gemma, two time queries** (`invoices I sent last year` 0.37 → 0.29, `photos from last summer` 0.09 → 0.08) move for the same reason: the word forms `invoice` and `photo` bring in more keyword matches. The time category still gains overall (0.736 → 0.741). In the app, the natural-language layer turns both into filters (`in:sent date:"last year"`, `has:image date:"last summer"`; tests in `nl.test.ts`).
- **Hybrid stand-in, spanish −0.004.** One query (`casa castillo website quote`, 0.14 → 0.06) with the bag-of-words stand-in. That embedder only shows the plumbing works; the same query doesn't move with Gemma.

The four categories that guard exact search stay where they were: identifier 1.000, operator 0.959, name 0.981 and natural 0.921 (keyword). With Gemma they are 0.997, 0.967, 0.964 and 0.987. The one below 0.96 on keyword is natural, at the same score as before; its misses are paraphrases (docs/SEARCH-RANKING.md).

### How the loop ran

Each step:
1. Run keyword and hybrid (Gemma).
2. Take the worst category.
3. Find the root cause in the parser, the lexicon, the dates, label resolution, ranking or the natural-language layer.
4. Fix it with unit tests.
5. Diff against the previous run.

A step was redone whenever any category dropped. [SEARCH-CASES.md](SEARCH-CASES.md#what-was-fixed-and-why-it-failed) lists the fixes, what caused each failure, and what remains out of reach.

Eight queries had judgments that were too narrow. For example, every Madrid hotel booking answers `réservation hôtel madrid`, not only the planted one. Those judgments were widened in their own commits (`search-eval: …`), before the comparison above.

### Latency

**Setup.** `cargo run -p penguin-core --release --example bench_search` at 300,000 messages, base (`9c9df2c`) against after. Each side has its own freshly built database, run twice, interleaved, on the shared build box (load 3–4). The table takes the better of the two passes.

**The 66 queries the benchmark already had:**
- median p50 10.3 → 10.4 ms;
- median p95 15.3 → 16.2 ms;
- worst p95 117.1 → 118.7 ms.

Typical rows, before → after, p50 / p95 ms:
- `invoice`: 31.9 / 48.1 → 35.4 / 54.9
- `lease renewal`: 19.8 / 25.1 → 18.0 / 28.5
- `the`: 63.2 / 88.9 → 59.8 / 71.7
- `INV-20417`: 1.4 / 1.4 → 1.4 / 2.2

Hybrid (stand-in model and index), pooled p50 / p95: 23.3 / 64.4 → 24.4 / 67.1 ms.

**The seven queries added for word forms and formats.** Before, they returned nothing in under 2 ms. They now find their mail at the cost of an ordinary search.

| Query | Before p50 / p95 ms | After p50 / p95 ms | Conversations |
|---|---:|---:|---:|
| `quarterly reports` (plural) | 0.5 / 0.7 | 13.2 / 17.8 | 0 → 50 |
| `colour meeting` (British spelling) | 1.8 / 2.3 | 19.8 / 30.5 | 0 → 50 |
| `leaserenewal` (unknown compound) | 0.5 / 0.6 | 5.8 / 7.4 | 0 → 5 |
| `INV20417` (code without its dash) | 0.5 / 0.6 | 1.3 / 2.1 | 0 → 2 |
| `4155550138` (digits without separators) | 0.5 / 0.9 | 1.3 / 2.0 | 0 → 0 |
| `invioce` (typo) | 0.5 / 0.6 | 23.2 / 35.1 | 0 → 50 |
| `reports deadline quarterly` | 1.8 / 3.9 | 18.0 / 21.1 | 0 → 50 |

**Typing.** As-you-type p95:
- "invoice from:mike": 84–86 → 63–65 ms;
- "that thing about the lease renewal", keyword: 108–115 → 87–99 ms.

**The lexicon's own cost.** The lexicon stage (`search stages=… lexicon=`) is under 1 ms for plain words and about 6 ms for a word it corrects. Two things keep it there:
- words are looked up in OR batches, halved while one exists;
- a word in 500+ messages is never checked as a split. "invoice" as "in voice" cost 9–20 ms.

When the words as typed already fill the candidate window, the second walk over their other forms is skipped.

**On the 19k-message harness:** keyword p50 / p95 over all 570 queries went from 0.6 / 3.9 to 1.6 / 6.0 ms. Hybrid (Gemma) went from 33.0 / 44.9 to 31.0 / 40.9 ms, where the query embedding dominates.

## Limitations

- **Template text.** The synthetic text is less diverse than real mail. An embedding model may find the paraphrases easier (short, clean messages) or harder (little context) than real ones. Confirm wins on real mail before shipping them.
- **One author wrote the queries, the targets and the judgments.** The paraphrase queries were written to avoid the target's words, so they measure the vocabulary-mismatch problem specifically; they don't estimate how often it happens. Real-mail mode is the check.
- **Attachment contents are not generated.** Only file names are indexed today.
- **Latency comes from a warm, freshly built database of 19k messages.** Scale and cold-start behaviour are measured by `bench_search` / `bench` (`docs/PERFORMANCE.md`).
- **The hybrid reference filters through a keyword search capped at 500 conversations.** For very broad filters, it can miss vector candidates.

## Sources

The IR sources below were checked against primary copies (paper PDFs, NIST proceedings, the trec_eval source). The facts used above are the ones those copies state.

- Järvelin, K. & Kekäläinen, J. (2002). Cumulated gain-based evaluation of IR techniques. *ACM TOIS* 20(4):422–446. doi:10.1145/582415.582418. The DCG/nDCG definitions and graded relevance on a 0–3 scale. trec_eval's `ndcg_cut` differs from the paper in one way: it discounts from rank 1 with log2(rank+1).
- trec_eval, the NIST evaluation tool: https://github.com/usnistgov/trec_eval.
  - `m_ndcg_cut.c`: "Gain values are the relevance values in the qrels file"; discount `log2(i + 2)` for 0-based rank *i*; the ideal comes from all judged documents.
  - `m_recip_rank.c` / `m_recall.c` with `-l` (the minimum grade that counts as relevant; default 1).
- Burges, C. et al. (2005). Learning to rank using gradient descent. *ICML*, 89–96. doi:10.1145/1102351.1102363. The exponential-gain variant (2^rel − 1), which trec_eval and this harness don't use.
- Voorhees, E. (1999). The TREC-8 Question Answering Track Report. *TREC-8*, NIST SP 500-246. https://trec.nist.gov/pubs/trec8/papers/qa_report.pdf. The definition of reciprocal rank and its mean.
- Thakur, N., Reimers, N., Rücklé, A., Srivastava, A. & Gurevych, I. (2021). BEIR: A Heterogeneous Benchmark for Zero-shot Evaluation of Information Retrieval Models. *NeurIPS Datasets and Benchmarks*. arXiv:2104.08663. Why nDCG@10 is the primary metric (§3.3), use of pytrec_eval, Hole@10 for unjudged results.
- Craswell, N., Mitra, B., Yilmaz, E., Campos, D. & Voorhees, E. (2020). Overview of the TREC 2019 Deep Learning Track. arXiv:2003.07820. The four-level scale and its wording, and NDCG@10 as the main metric.
- Craswell, N., de Vries, A. & Soboroff, I. (2005). Overview of the TREC-2005 Enterprise Track. https://trec.nist.gov/pubs/trec14/papers/ENTERPRISE.OVERVIEW.pdf. The known-item and discussion search tasks over W3C mailing lists: known-item scored by MRR and S@10, discussion search judged on three levels.
- Soboroff, I., de Vries, A. & Craswell, N. (2006). Overview of the TREC 2006 Enterprise Track. https://trec.nist.gov/pubs/trec15/papers/ENT06.OVERVIEW.pdf. Email discussion search with graded judgments by NIST assessors.
- Wang, X., Bendersky, M., Metzler, D. & Najork, M. (2016). Learning to Rank with Selection Bias in Personal Search. *SIGIR*, 115–124. doi:10.1145/2911451.2911537. Why third-party relevance judgments are infeasible for personal mail.
- Ai, Q., Dumais, S., Craswell, N. & Liebling, D. (2017). Characterizing Email Search using Large-scale Behavioral Logs and Surveys. *WWW*, 1511–1520. doi:10.1145/3038912.3052615. People "often look for specific known items" with short queries.
- Elsweiler, D. & Ruthven, I. (2007). Towards task-based personal information management evaluations. *SIGIR*, 23–30. doi:10.1145/1277741.1277748. A diary study of re-finding in email: lookup, known-item and multi-item tasks; task-based evaluation.
- Carmel, D., Halawi, G., Lewin-Eytan, L., Maarek, Y. & Raviv, A. (2015). Rank by Time or by Relevance? Revisiting Email Search. *CIKM*, 283–292. doi:10.1145/2806416.2806471. The time-versus-relevance tension in email ranking (hence the `recency` facet).
- Dumais, S. et al. (2003). Stuff I've Seen: A system for personal information retrieval and re-use. *SIGIR*, 72–79. doi:10.1145/860435.860451.
- Kim, J. & Croft, W. B. (2009). Retrieval experiments using pseudo-desktop collections. *CIKM*, 1297–1306. doi:10.1145/1645953.1646117. Synthetic desktop and email collections with generated known-item queries, scored by MRR.
- Azzopardi, L., de Rijke, M. & Balog, K. (2007). Building simulated queries for known-item topics. *SIGIR*, 455–462. doi:10.1145/1277741.1277820.
- Smucker, M., Allan, J. & Carterette, B. (2007). A comparison of statistical significance tests for information retrieval evaluation. *CIKM*, 623–632. doi:10.1145/1321440.1321528. Use the randomization test; the sign and Wilcoxon tests should not be used.
- Buckley, C. & Voorhees, E. (2004). Retrieval evaluation with incomplete information. *SIGIR*, 25–32. doi:10.1145/1008992.1009000. Incomplete judgments, bpref.
- Voorhees, E. & Buckley, C. (2002). The effect of topic set size on retrieval experiment error. *SIGIR*, 316–323. doi:10.1145/564376.564432. With the TREC Robust track follow-up (Voorhees, *SIGIR Forum* 39(1), 2005), which gives error rates by topic-set size.
- Cormack, G., Clarke, C. & Büttcher, S. (2009). Reciprocal rank fusion outperforms Condorcet and individual rank learning methods. *SIGIR*, 758–759. doi:10.1145/1571941.1572114. RRF with k = 60.
- Zhang, X. et al. (2023). MIRACL: A Multilingual Retrieval Dataset Covering 18 Diverse Languages. *TACL* 11:1114–1131. doi:10.1162/tacl_a_00595. It includes Spanish: use it for a public multilingual check of the embedding model, alongside this harness.
- ir_measures documentation, `Judged@k`: https://ir-measur.es/en/latest/measures.html.
