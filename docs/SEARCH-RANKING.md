# Search ranking: keywords and meaning

Penguin's search box answers two kinds of questions with one ranking. Known-item lookups ("INV-20417", "from:mike lease", "pinecone42") need exact matches. Remembered-content questions ("that thing about ending the lease early", "plane tickets to portugal") need matches by meaning, often with none of the typed words in the message. Everything runs on the Mac. The keyword side is SQLite FTS5; the meaning side is a local embedding model and vector index (`penguin-semantic`). Users never learn syntax, and every operator behaves exactly as documented in [SEARCH.md](SEARCH.md).

Code: `crates/penguin-core/src/hybrid.rs` (rewrites, routing, the meaning side, fusion, passages) on top of `search.rs` (parsing, filters, the keyword side). Tests: `hybrid_tests.rs`. Measurements:
- Quality: the shared relevance harness, `penguin-eval --mode penguin` ([SEARCH-EVAL.md](SEARCH-EVAL.md)), and this change's own small set, `examples/hybrid_eval.rs`.
- Speed: the hybrid section of `examples/bench_search.rs`.

## What happens to a query

0. **Rewrite plain words** (only with meaning on; `hybrid::rewrite`):
   - **Dates typed as words.** "dentist last spring" becomes "dentist" plus a soft date window. The window comes from the same grammar as `date:` (`query::date_hint`, which also drives the "Use … as a date?" hint). Mail inside the window gets +0.30; mail outside gets a Gaussian share of that, with σ = half the window and at least a week. Shortwave boosts extracted date ranges the same way. `date:` itself stays a hard filter, and a date alone ("last spring") is left to the parser.
   - **Misspellings.** A word of 4+ letters that no message contains makes the all-words keyword match empty. It is replaced by an indexed word one edit away (insertion, deletion, substitution or swap), found by generating those candidates, as Norvig's corrector does, and checking each with a seek in the full-text index; there is no vocabulary scan. Damerau (CACM 1964) found about 80% of misspellings are one such edit. A candidate that occurs together with the query's other words wins ("recipt deskcraft" → "receipt", not "recipe"), then the most frequent. The keyword side always uses the correction. The model gets it only when the other words confirm it: an unknown word is often a real word the mail doesn't use ("plane" where the mail says "flight"), and "correcting" it to "place" would change the meaning.
1. **Parse.** The existing parser turns operators into filters and chips. The free text left over, in the order typed (`from:mike that thing about the lease` → `that thing about the lease`), is what the model embeds (`semantic_text`). While a word is still being typed and is under 3 characters, it is left out.
2. **Route.** The shape of the query sets α, the weight meaning gets (table below).
3. **Keyword side.** The same bm25 walk as before this change: all words required, subject/sender/filename weighted, newest 5,000 candidates.
4. **Meaning side.** The query vector goes to the vector index for the nearest 100 chunks. **Every filter applies to both sides.** Accounts and dates are checked inside the index search (`ChunkRef.account_id`, `date`). Other filters depend on the set they select:
   - When an index can produce the set cheaply (`from:`/`to:`/`subject:`/`domain:` through FTS, `label:`, `is:new-sender`, `is:unread`, `filename:`) and it holds at most 20,000 messages, it is built first and the vector search only visits messages in it (pre-filtering).
   - A broad flag filter (`in:sent`, `has:pdf`) is checked after the search instead, on 4× as many chunks.
   - Either way, each candidate message is checked against the same SQL predicates, FTS field filters and exclusions (`-word`, `-from:x`) the keyword side uses, and Trash and Spam stay out unless asked for.
5. **Fuse** (below), collapse per conversation (the best message represents it, +0.03·ln(matches)), then rerank the top 200 conversations with thread-level signals.
6. **Snippet and passage.** A conversation found only by meaning shows its best passage as the snippet: HTML-escaped, with any typed words that occur marked. Every conversation matched by meaning also carries `passage` (plain text, ≤240 characters).

With no model (`semantic: "off"`), with the index still being built (`"indexing"`, plus `semanticProgress`), when the model fails, or on a route without meaning, none of this runs. Search is then byte-for-byte the keyword search; a test asserts identical pages.

## Routing

| Query shape | Example | α (meaning weight) | Why |
|---|---|---:|---|
| Filters only | `from:mike is:unread` | off | Nothing to embed. |
| Quoted phrase, or text inside `OR` / `( )` | `"early termination"`, `lease OR rent` | off | Exact syntax asks for exact results. |
| Only names of people you correspond with | `mike delgado` | off | A name is an identity, not a meaning. Dense retrievers are weak on entities: 49.7% vs BM25's 72.0% top-20 accuracy on EntityQuestions (Sciavolino et al. 2021). The keyword side already boosts that person's mail. |
| A code, number or address | `INV-20417`, `order 88213`, `ana@ruiz.example` | 0.25 | Identifiers have no meaning to embed. Pinecone suggests α≈0.25 "for queries with high keyword specificity". Keyword hits also get +0.15. |
| One word | `escrow` | 0.25 | Short known-item queries are the norm in email (Ai et al., WWW 2017). |
| Two plain words | `lease renewal` | 0.5 | Balanced. |
| Three or more content words whose words all occur together in some mail, with no question or cue | `chicago expense report` | 0.5 | The words are what the user remembers, so the keyword side counts as much as meaning. |
| Three or more content words, a question opener (what/when/where/who/how/did…), `?`, or a descriptive cue ("thing about", "the email where", "looking for"…) | `that thing about ending the lease early` | 0.7 | Natural-language descriptions. Bruch et al. found α∈[0.6, 0.8] "to consistently lead to improvements"; Pinecone suggests 0.75 for natural-language queries. |

A keyword-led query whose keyword side already found 20 or more conversations skips the model altogether. This is a latency guard: the typed words found plenty, and at α 0.25 meaning would only reorder the tail. It never changed a result in the eval.

## Fusion

Per message:

```
relevance = (1 − α) · kw + α · sem          (convex combination)
score     = relevance + priors + boosts
```

- `kw` = −bm25 / the best −bm25 for this query: FTS5's bm25 with k1 = 1.2, b = 0.75 (fixed by SQLite), sign flipped because "better matches are assigned numerically lower scores" ([FTS5 docs](https://www.sqlite.org/fts5.html)). Normalizing against the best match is theoretical min-max with bm25's minimum of 0, which is Bruch et al.'s TM2C2 for the lexical side. This is unchanged from keyword-only search.
- `sem` = the message's best chunk similarity, min-max normalized over the retrieved list, (s − s_min)/(s_max − s_min). The best chunk is 1 and the list's weakest, standing in for "unrelated mail" at this query, is 0. Meaning-only candidates below 0.2 are dropped; from 0.5 the conversation counts as "matched by meaning" (the tag and the passage).
- A message found by only one side gets that side's share. **Agreement wins:** a message both sides rank first scores 1.

**Why convex combination, and why min-max on the meaning side.** Cormack et al.'s reciprocal rank fusion (RRF, SIGIR 2009) is the default in Elasticsearch, OpenSearch, Azure AI Search and Vespa, all with k = 60. It needs no score calibration, which made it the obvious first candidate. Bruch, Gai and Ingber (ACM TOIS 2023, "An Analysis of Fusion Functions for Hybrid Retrieval") found a convex combination "outperforms RRF in in-domain and out-of-domain settings", found RRF "sensitive to its parameters", and found α converges with "a small set of queries". Their argument is that RRF "ignores the raw scores and discards information about their distribution".

Mail search adds query-independent priors on top (recency, starred…), and the scale of the fused relevance decides how much those priors weigh. RRF's spread is small: with k = 60, rank 1 and rank 20 differ by 1/61 vs 1/80. The same holds for theoretical min-max of cosine, (s+1)/(s_max+1): small models put most mail within 0.3 cosine of the best match. In both cases recency ends up ranking the results. Min-max over the retrieved list spreads meaning across [0, 1] whatever the model's cosine range. All three were measured, and min-max won on both real models (table below).

**Priors** (unchanged from keyword search, applied to messages from either side): 0.35 · recency, with recency = 1/(1 + age/90 days); starred +0.10; unread +0.03; a sender you write to, up to +0.12 (log of messages sent to them); a sender a typed word names +0.30. Freshness is the strongest feature in Yahoo Mail's learned ranker (Carmel et al., CIKM 2015: "Freshness >> User actions >> Similarity >> Sender features"). Gmail's relevance search names "recency, most-clicked emails and frequent contacts" ([Google, 2025](https://blog.google/products/gmail/gmail-search-update-relevant-emails/)). Apple Mail's Top Hits reflect "messages you've read and replied to recently, your VIP senders and contacts" ([Apple](https://support.apple.com/guide/mail/search-for-emails-mlhlp1003/mac)).

**Hybrid-only signals** (rerank of the top 200 conversations, and per message):

| Signal | Weight | Reason |
|---|---:|---|
| You sent a message in the thread | +0.05 | Threads you took part in are yours (Apple's "replied to"; Carmel's "user actions"). |
| Gmail marked it important | +0.03 | A learned signal of Gmail's own; kept small. |
| The subject contains the query's content words in order (2+ words) | +0.10 | Exact subject phrase. |
| The sender's name is exactly the query's words (2+ words) | +0.30 | Exact sender name (the single-word case is the existing +0.30 prior). |
| The query has a code and the message matched by words | +0.15 | The message holds the identifier. |
| Bulk mail (List-Unsubscribe, Promotions/Updates/Forums) matched by meaning | −0.15 | Newsletters are topically close to everything. A travel digest mentioning Lisbon is near "plane tickets to portugal", but it is rarely what someone searching their own mail wants. |

## Measured quality

### The shared harness (200 queries, 19,183 messages)

`penguin-eval` ([SEARCH-EVAL.md](SEARCH-EVAL.md)) has 200 judged queries over a 19,183-message synthetic mailbox. `--mode penguin` runs exactly `Store::search_hybrid`, over a vector index cut by `chunk_message`. Real vectors come from `--embedder file:…`, made offline by `search-eval texts` plus `examples/ranking/embed.py`:

```sh
cargo run -p penguin-eval --release -- texts --out target/search-eval/texts.json
(cd target/search-eval && uv run --python 3.12 --with fastembed python \
   ../../crates/penguin-core/examples/ranking/embed.py texts.json . BAAI/bge-small-en-v1.5)
cargo run -p penguin-eval --release -- run --mode keyword --name keyword
cargo run -p penguin-eval --release -- run --mode penguin --embedder file:target/search-eval/vectors-bge-small-en-v1.5.json --name penguin-bge
cargo run -p penguin-eval --release -- diff target/search-eval/keyword.json target/search-eval/penguin-bge.json
```

nDCG@10, keyword search (before) against hybrid, same day and same build, 2026-09-27:

| Category | n | Keyword (before) | Hybrid, bge-small | Hybrid, MiniLM-L6 |
|---|---:|---:|---:|---:|
| identifier | 23 | 1.000 | 1.000 | 0.997 |
| operator | 26 | 0.959 | 0.963 | 0.966 |
| name | 17 | 0.981 | 0.991 | 0.996 |
| natural | 24 | 0.921 | 0.971 | 0.966 |
| paraphrase | 33 | 0.000 | **0.617** | **0.597** |
| question | 28 | 0.107 | **0.687** | **0.747** |
| misspelling | 17 | 0.059 | **0.810** | **0.850** |
| spanish | 18 | 0.346 | **0.628** | **0.564** |
| time | 14 | 0.286 | **0.672** | **0.621** |
| **all** | 200 | 0.505 | **0.811** | **0.810** |

- bge-small: 81 queries better, 1 worse. The loss is `chicago expense report`, 1.00 → 0.63: other "Expense report" threads also contain all three words and are newer. The five categories aimed at gain significantly (paired randomization test, p ≤ 0.008); natural gains +0.050 but not significantly (p = 0.37).
- MiniLM: 78 better, 2 worse.
- Queries with no results at all: 93 → 0.
- The four categories keyword search already served (identifier, operator, name, natural) all stay ≥ 0.96.

On the harness, three changes each fixed a measured problem:
- **Embedding the words as typed.** Paraphrase went from 0.497 to 0.604 on bge, because the corrector had been turning real words into indexed neighbours.
- **The context check for corrections.** Misspelling went from 0.767 to 0.810 on bge.
- **The balanced rule for long plain queries** fixed `chicago expense report`, which fell to 0.50 without it.

### This change's own set (44 queries)

`cargo run -p penguin-core --release --example hybrid_eval -- --verbose --sweep`

The set: 63 fictional messages in 52 conversations, the mail of one person: a lease, a trip, a dentist, a plumber, taxes, work, family, and newsletters written as distractors. There are 44 queries judged per conversation (2 = the answer, 1 = related), all written before any tuning:
- 10 codes and names
- 8 two-word queries
- 20 descriptive queries and questions, most of which use none of the answer's words
- 6 with operators

Vectors come from real models: [BAAI/bge-small-en-v1.5](https://huggingface.co/BAAI/bge-small-en-v1.5) and [all-MiniLM-L6-v2](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2), 384 dimensions, through fastembed's ONNX export. They are precomputed by `examples/ranking/embed.py` and stored int8 in the repo, so the eval needs no model to run. The hash stand-in (`HashEmbedder`, shared words only) is shown as a floor.

**The set deliberately over-represents vocabulary mismatch:** keyword search finds nothing relevant for 26 of the 44 queries. Read the absolute numbers as "what meaning adds where words fail", not as expected everyday gains. The column that guards everyday use is **keyword**: exact lookups must not get worse.

| Ranking | nDCG@10 | MRR@10 | R@10 | nDCG keyword | nDCG words | nDCG meaning | nDCG filtered |
|---|---:|---:|---:|---:|---:|---:|---:|
| Keyword only (before) | 0.387 | 0.409 | 0.375 | 0.976 | 0.578 | 0.000 | 0.442 |
| **Hybrid, bge-small** | **0.956** | **0.970** | **0.989** | 0.976 | 0.966 | 0.929 | 1.000 |
| **Hybrid, MiniLM-L6** | **0.940** | **0.960** | **0.977** | 0.976 | 0.922 | 0.919 | 0.971 |
| Hybrid, hash stand-in | 0.723 | 0.716 | 0.830 | 0.976 | 0.674 | 0.551 | 0.942 |

Keyword lookups are unchanged (0.976 on every row). Everything else gains.

Alternatives, bge-small / MiniLM mean nDCG@10:

| Variant | bge-small | MiniLM |
|---|---:|---:|
| Shipped: convex, min-max, routed α | **0.956** | **0.940** |
| Convex, theoretical min-max (TM2C2) | 0.697 | 0.853 |
| Weighted RRF, k = 60 | 0.812 | 0.800 |
| Weighted RRF, k = 5 | 0.945 | 0.933 |
| Fixed α = 0.5, no routing | 0.941 | 0.937 |
| Fixed α = 0.7, no routing | 0.957 | 0.923 (keyword queries drop to 0.902) |
| No thread-level signals | 0.940 | 0.917 |

Routing matters most for the weaker model on exact lookups: a fixed α = 0.7 costs MiniLM 0.074 nDCG on the keyword queries. The thread-level signals add 0.016–0.023.

Sensitivity, varying one parameter at a time. The defaults sit on plateaus, not peaks:
- α for descriptive queries: 0.941 → 0.958 over 0.5 → 0.9 (bge), flat from 0.7.
- α for two-word queries: flat over 0.5–0.7.
- Bulk-mail penalty: 0.941 at 0 → 0.956 at 0.15, flat to 0.25.
- Participation: best at 0.05.
- Semantic floor: flat over 0–0.3.

Where it still misses:
- "wedding invitation reply deadline" (nDCG 0.36): a 70-day-old invitation loses to a 1-day-old verification code the model finds similar ("expires").
- "when is my teeth cleaning" (0.63): a pet-dental newsletter that actually says "teeth cleaning" matches by words and ranks above the dentist's reminder, which says only "cleaning".

This set is small and was written by the same hand as the ranking, so treat it as a regression guard and a sanity check; the shared harness above is the arbiter.

## Measured speed

The model's query embedding, one query per call on this build box (Intel Haswell VM, 8 vCPU, shared with other builds), ONNX Runtime via fastembed, 440 calls:

| Model | 1 thread p50 / p95 | 4 threads p50 / p95 |
|---|---:|---:|
| bge-small-en-v1.5 | 14.2 / 20.4 ms | 20.5 / 39.0 ms |
| all-MiniLM-L6-v2 | 8.7 / 15.9 ms | 10.6 / 24.7 ms |

What hybrid search adds around the model and the index, at 300,000 messages: `PENGUIN_BENCH_ONLY=hybrid cargo run -p penguin-core --release --example bench_search`.
- The index is a stand-in with no vector math that calls the query's filter on the chunks it visits, as a filtered ANN search does, so its cost is not in these numbers.
- The model is a stand-in that returns instantly.
- Measured on the same shared VM under load (1-minute load 4–5). Keyword numbers here run 2–4× the documented ones in [PERFORMANCE.md](PERFORMANCE.md), so read the differences, not the absolutes.

| Query | Route | Keyword p50 / p95 ms | Hybrid p50 / p95 ms |
|---|---|---:|---:|
| `invoice` | keyword, 20+ hits (meaning skipped) | 32.2 / 51.0 | 35.1 / 52.4 |
| `INV-20417` | keyword, code | 3.1 / 4.1 | 8.6 / 9.9 |
| `lease renewal` | balanced | 21.9 / 38.0 | 29.2 / 41.7 |
| `that thing about the lease renewal` | semantic | 29.2 / 39.0 | 33.0 / 42.3 |
| `when is the budget review meeting` | semantic | 22.8 / 30.6 | 21.0 / 33.2 |
| `from:mike what did he say about the lease` | pre-filtered by an FTS field | 16.0 / 18.3 | 43.7 / 50.7 |
| `label:Label_3 notes from the planning meeting` | pre-filtered by a rowid set | 6.8 / 12.1 | 38.0 / 54.1 |
| `is:unread what did they say about the budget` | pre-filtered (unread index) | 20.0 / 30.3 | 26.2 / 41.9 |
| `in:sent budget planning thoughts` | checked after the search | 5.8 / 9.6 | 27.5 / 42.5 |
| `has:pdf the signed contract` | checked after the search | 5.3 / 9.7 | 18.3 / 26.3 |
| `lease renewal paperwork date:"last spring"` | date in the index filter | 6.2 / 10.4 | 18.7 / 24.7 |
| `"early termination clause"` | exact syntax (no meaning) | 0.7 / 1.0 | 0.7 / 1.0 |
| **pooled, 14 shapes** |  | 17.3 / 58.7 | 26.9 / 69.2 |
| typing `that thing about the lease renewal`, per keystroke |  | 38.9 / 104.3 | 43.3 / 115.5 |

**One call, not two phases.** Hybrid adds about 10 ms at p50 and p95 around the model and the index on this loaded VM. The model adds its query embedding: MiniLM p50 8.7 / p95 15.9 ms, bge-small p50 14.2 / p95 20.4 ms, single-threaded here. Apple silicon was not measured.

On the reference Mac, keyword search is 13 ms p95 at 300,000 messages. So keyword + hybrid work + a small model's embedding should stay at or under about 40–45 ms p95 there, inside the ~60 ms budget, as long as the real index answers a filtered top-100 in a few milliseconds (HNSW-class; a brute-force scan of 300k vectors would not).

Two phases (keyword results first, a fused list later) would keep keyword latency exactly as before. But every descriptive query would visibly reorder under the cursor, and the keyword work would run twice. Latency is protected instead where it matters:
- Exact syntax, filters and names never touch the model.
- A keyword-led query with 20+ keyword hits skips it.
- Query vectors are cached (64 recent texts), so deleting a word costs no embedding.

If the chosen model's embedding costs more than ~25 ms p95 on the Mac, switch to two phases:
- `search` returns keyword results with `semantic: "pending"`.
- A second command returns the fused page for the same request id.

The slowest hybrid shapes are selective filters (`from:`, `label:`) with free text. Building the pre-filtered id set, up to 20,000 messages, costs about 25 ms here. A lower cap (5,000) would trade it for post-filtering, which can miss matches when the filter is very selective.

Before the fixes in this change, some shapes were far slower (in:sent 480 ms, has:pdf 285 ms, date: 147 ms, label: 168 ms). A vocabulary scan for misspellings was the main cost; fts5vocab's 'row' table merges every posting list to count documents. The fix replaced the scan with one-edit candidates checked in the index, and candidates are now looked up by rowid after one unique-index seek each.

## API

- `SearchHit.matchedBy: ("words" | "meaning")[]` (Rust `matched_by: Vec<String>`): empty for filter-only queries and "Also search Gmail" results.
- `SearchHit.passage: string | null`: the best passage by meaning, plain text, ≤240 characters, when `matchedBy` includes `"meaning"`.
- `SearchResponse.semantic: "ready" | "indexing" | "off"` and `semanticProgress: number | null` (0–1 while indexing).
- `Store::search` is keyword search. `Store::search_hybrid(request, Option<&SemanticHandles>)` is what the `search` command calls, with the handles from `AppState::semantic()`.
- `SemanticHandles { embedder, index, progress }`: progress below 1 means indexing.

The contract with the indexer:
- Chunks are cut by `penguin_semantic::chunk_message(subject, authored_body)`, where the authored body is `penguin_core::text::strip_quoted(body_text)`, or the snippet for headers-only mail.
- `ChunkRef.chunk` indexes that list; search re-cuts the same message to find a hit's passage.
- `ChunkRef.date` is the message date in epoch ms, and `account_id` / `message_id` are the store's ids, so date and account filters run inside the index.

## Sources

- Cormack, Clarke, Büttcher. *Reciprocal Rank Fusion outperforms Condorcet and individual Rank Learning Methods.* SIGIR 2009. http://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf. k = 60 was "near-optimal, but the choice was not critical" (MAP 0.2145 at k = 60 vs 0.2147 at 80).
- Bruch, Gai, Ingber. *An Analysis of Fusion Functions for Hybrid Retrieval.* ACM TOIS 2023. https://arxiv.org/abs/2210.11934. Convex combination beats RRF on every dataset in their Table 2 (MS MARCO NDCG 0.454 vs 0.425); α ∈ [0.6, 0.8]; sample efficient.
- Thakur et al. *BEIR.* NeurIPS 2021 Datasets. https://arxiv.org/abs/2104.08663. "BM25 remains a strong baseline for zero-shot text retrieval"; several dense models fall below BM25 out of domain.
- Muennighoff et al. *MTEB.* 2022. https://arxiv.org/abs/2210.07316. "No particular text embedding method dominates across all tasks."
- Chen et al. *Out-of-Domain Semantics to the Rescue! Zero-Shot Hybrid Retrieval Models.* ECIR 2022. https://arxiv.org/abs/2201.10582. Hybrid +20.4% over dense and +9.54% over lexical out of domain.
- Sciavolino et al. *Simple Entity-Centric Questions Challenge Dense Retrievers.* EMNLP 2021. https://arxiv.org/abs/2109.08535.
- Carmel et al. *Rank by Time or by Relevance? Revisiting Email Search.* CIKM 2015 (Yahoo Mail). Feature importance as reported in the authors' slides.
- Carmel et al. *Promoting Relevant Results in Time-Ranked Mail Search.* WWW 2017.
- Ai, Dumais, Craswell, Liebling. *Characterizing Email Search using Large-scale Behavioral Logs and Surveys.* WWW 2017. Email queries are short known-item lookups, "much less likely to search for general information on a topic".
- Wang et al. *Learning to Rank with Selection Bias in Personal Search.* SIGIR 2016. Personal search has too few clicks per query for click models, which argues for a handful of global parameters rather than learned per-query ones.
- SQLite FTS5 bm25: https://www.sqlite.org/fts5.html.
- Vendor defaults: Elasticsearch RRF `rank_constant` 60 (https://www.elastic.co/docs/reference/elasticsearch/rest-apis/reciprocal-rank-fusion); Azure AI Search RRF k = 60 (https://learn.microsoft.com/en-us/azure/search/hybrid-search-ranking); Vespa `reciprocal_rank` default 60 (https://docs.vespa.ai/en/phased-ranking.html); Weaviate `relativeScoreFusion` (min-max then weighted sum) as default since v1.24 (https://docs.weaviate.io/weaviate/search/hybrid); Pinecone convex combination with α ≈ 0.25 for keyword-specific and ≈ 0.75 for natural-language queries (https://docs.pinecone.io/guides/search/hybrid-search/single-index).
- How mail apps rank, from their own words:
  - Gmail: https://blog.google/products/gmail/gmail-search-update-relevant-emails/
  - Apple Mail: https://support.apple.com/guide/mail/search-for-emails-mlhlp1003/mac
  - Shortwave, cloud: embeddings and full-text search, a cross-encoder rerank, a Gaussian boost inside extracted date ranges, 3–5 s end to end: https://www.shortwave.com/blog/deep-dive-into-worlds-smartest-email-ai/
  - Superhuman, local full-text search: https://blog.superhuman.com/delightful-search-more-than-meets-the-eye/
  - Fastmail, Xapian; ranking not described: https://www.fastmail.com/blog/email-search-system/
  - HEY: nothing published.

Penguin takes their signals (recency, contacts, interaction), keeps everything on device, and answers in milliseconds, not seconds.

## Open questions

- **Misspellings with bge-small.** bge embeds a typo far from the word it meant ("recipt" lands near "recipe"), so a correction the query's other words don't confirm leaves the model with the typo. MiniLM tolerates typos better (misspelling 0.850 vs 0.810). Model choice matters here.
- **Soft dates only with meaning on.** Respelling now runs in keyword search too (`lexicon.rs`, with inflections, British/American spellings, compounds and number formats), since it only fires when a word matches no message. Soft dates still run only in hybrid search.

- **Search while indexing.** Until the index covers the mail, search is keyword-only, as specified. The index is presumably built newest first, so meaning results over the part already indexed would help sooner. That is a one-line change in `search::search`; the UI would need to say the results are partial.
- **Tuning with real use.** α per route and the prior weights are global constants chosen on this set and the literature. Personal search has too few clicks per user to learn per query (Wang et al. 2016). A local, private log of which result was opened could tune one or two globals later.
- **The model's cosine scale.** Min-max makes fusion model-agnostic but always calls the best chunk a 1, even when nothing relevant exists. A per-model absolute floor, once the model is chosen, would let a query with no good meaning match show none.
