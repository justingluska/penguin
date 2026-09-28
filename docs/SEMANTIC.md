# Search by meaning

Penguin's keyword search finds the words you type. Search by meaning also finds mail that says the same thing in other words, or in another language. It runs entirely on the Mac: a small embedding model turns each email into a few vectors once, in the background, and each search turns the query into one vector and looks for the nearest ones. No mail, query or vector leaves the computer. The only network traffic is a one-time download of the model files from Hugging Face, pinned to an exact commit and checked byte for byte.

This document explains what it does for you, then every decision behind it with the evidence and sources, then the measurements.

- Code: `crates/penguin-semantic` (chunking, model, embedder, vector index), `apps/desktop/src-tauri/src/semantic/` (download, background indexer, power policy, `semantic_status`), `crates/penguin-core/src/store_semantic.rs` (what the indexer reads).
- Ranking (merging these results with keyword results) is not here: it lives in penguin-core's search. The hand-off is described under [How search uses it](#how-search-uses-it).

## What it finds that keyword search misses, and the reverse

Keyword search (SQLite FTS5) is exact and predictable: every word you type must appear. That is exactly right for names, codes and phrases, and exactly wrong when you remember what an email was about but not how it was worded. The two are complementary; search by meaning is added to keyword search, never a replacement for it.

Each example below comes from Penguin's evaluation set (`crates/penguin-semantic/tests/fixtures/email_eval.json`: fictional mail in English, Spanish and Tagalog, with look-alike decoys). "Rank" is where the right email landed.

**Meaning finds it, words don't:**

1. **"plane tickets to Portugal"** finds *"Booking confirmed: TP204 New York to Lisbon"*. The email never says plane, tickets or Portugal. Search by meaning: rank 1. Keyword ranking: rank 19 (and Penguin's keyword search, which requires every word, returns nothing).
2. **"why did the site go down"** finds the postmortem *"the primary database ran out of disk … the API returned 500s for 47 minutes"*. Meaning: rank 1; words: rank 22.
3. **"grandma's birthday party"** finds both *"Cumpleaños de la abuela"* (Spanish) and *"Kaarawan ni Lola"* (Tagalog). Meaning: rank 1; words: rank 49. The model maps all three languages into one space, so an English query finds Spanish and Tagalog mail and the other way round.
4. **"leak from the neighbor upstairs"** finds the insurance claim *"water damage to the bathroom ceiling caused by the unit above"*. Meaning: rank 1; words: rank 14.
5. **"deuda con el mecánico"** (a debt with the mechanic) finds *"Factura pendiente de pago … reparación del coche"*. Meaning: rank 1; words: rank 54.
6. **"house tour this weekend"** finds *"I booked a walkthrough of the 3 bedroom on Birch for Sunday"*. Meaning: rank 1; words: rank 33.

Over the whole set (52 queries, 106 messages including decoys), the right email is first 94% of the time by meaning versus 48% by words, and in the top 5 99% versus 69%.

**Words find it, meaning doesn't (or shouldn't be trusted):**

1. **Negation.** `-dentist` removes the dentist email. Typed as meaning, **"not the dentist"** ranks the dentist email *first*: embeddings don't understand "not".
2. **Numbers and codes.** For **"invoice 1038"**, meaning scores the right email 0.49 and *"Invoice 1042 past due"* 0.46: to a model, one invoice number is much like another. Keyword search matches `1038` exactly. The same goes for order numbers, booking codes (`ABX7KQ`), tracking numbers and amounts.
3. **Names and companies.** **"Northwind"** by words finds all three Northwind emails. By meaning, the overdue Northwind invoice isn't in the top three: the name is a small part of each email's vector. Published email research shows the same: on EnronQA (103k real emails), plain BM25 keyword retrieval beat a neural retriever (Recall@5 87.5% vs 59.4%), which the authors attribute to names and proper nouns ([EnronQA, arXiv 2505.00263](https://arxiv.org/abs/2505.00263)).
4. **Nothing relevant exists.** Meaning always returns *something*: for **"TP204"** the booking is first, then *"New laptop approved"* and *"Leo turns 6!"*, which have nothing to do with it. Keyword search returns exactly one email. This is why meaning results are ranked alongside keyword results, not shown alone.
5. **Filters and counts.** "From Dana", "last week", "has a PDF" and "how many invoices" are exact questions. Operators (`from:`, `date:`, `has:pdf`) answer them completely; meaning only ranks by similarity. Account and date filters are applied *before* the nearest-neighbour cut, so "best matches from this account" is correct, not the global best minus the rest.

## How it works

```
mail DB (penguin.db) ──► indexer thread (utility QoS, throttled by power/thermal state)
    newest first            │  store.semantic_rows / semantic_texts  (authored text, quotes stripped)
                            │  chunk_mail: chunk 0 = subject + sender + attachments + opening; then body windows
                            │  OnnxEmbedder.embed_passages  (EmbeddingGemma, 4-bit, ONNX Runtime CPU)
                            ▼
                      semantic.db (int8 vectors + docs)  ◄──►  SemanticIndex in RAM (int4 codes + 20 B/chunk)
                                                                     ▲
search_hybrid ── AppState::semantic() ── embed_query ──► search_where(query, k, filter)  (fused with FTS5 in penguin-core)
```

- **Chunking** (`chunk.rs`). Chunk 0 is the message's identity card: subject, sender, attachment names, then the first 90 words of the authored text. The rest of the authored text becomes 120-word windows overlapping by 20 words, each prefixed with the subject, at most 8 chunks per message. Newsletters and other bulk mail (List-Unsubscribe or the Promotions/Updates/Forums categories) get chunk 0 only. The text is the quote-stripped authored part that the keyword index already uses (`text::strip_quoted`: reply history out, forwarded content kept, since it is often what you search for). Search's passage display re-cuts the same text with `chunk_message`, so chunk numbers always agree. URLs are reduced to their host, and signatures after `-- `, "Sent from my iPhone" lines (English, Spanish and Tagalog variants), unsubscribe and view-in-browser footers, and long opaque tokens are dropped. Headers-only messages (older than the sync window) are embedded from their snippet and re-embedded when their body arrives. Windows are counted in words so chunking is independent of the model. At about 1.3–2 tokens per word, 120 words stays under the model's 256-token limit set here. Research guidance was 250–500-token chunks with ~50 tokens of overlap, "prefixed with subject and minimal sender/date context" ([research/02 §4](../research/02-search-deep-dive.md)). Penguin uses smaller windows because each window costs compute and a smaller window is a more specific match.
- **What is stored.** One `docs` row per message (account, ids, date, mail-DB rowid, a change signature, and the model id) and one `chunks` row per passage (int8 codes plus the scale), in `<data dir>/semantic.db`. That is a separate SQLite file, so vectors never force a migration of the mail database and can be deleted and rebuilt freely. The model id covers the model, commit, dimensions, token limit and chunker version. Opening with any other id drops all vectors, and the indexer re-embeds.
- **Search** (`index.rs`). Every chunk is kept in RAM as int4 codes (128 bytes at 256 dimensions, plus a 4-byte scale), with 16 bytes of metadata (message, chunk, account, date). That is 148 bytes a chunk, or 89 MB at 600k chunks. The query is quantized to int8. Then:
  1. Scan every row that passes the account/date filter, scoring the int8 query against the int4 codes, and keep the best `max(4·k, 200)`, on up to 4 threads for large indexes.
  2. Rescore those with the int8 vectors read from semantic.db.
  3. Return the top k, or with `search_messages`, the best chunk per message.

  The numbers that justify this are under [Measurements](#measurements).
- **Keeping up.** One background thread owns all of it:
  - **Download and load.** It downloads (first run), verifies and loads the model. On a new install the download waits for the Welcome setup: it starts when search by meaning is confirmed on there, or when the setup is finished or skipped (docs/ONBOARDING.md). Until then the state is `paused`.
  - **Sweep.** It compares the mail database with semantic.db page by page (500 rows, newest first), embeds what's missing or changed, and deletes vectors whose message is gone, now spam or a draft. Its cursor is saved after every page, so quitting mid-backfill resumes there.
  - **New mail.** A `mail_changed` from sync runs a short pass over everything newer than the last pass, so new mail is searchable by meaning seconds after it arrives. A full pass (cheap when nothing changed) runs every 30 minutes to catch deletions deep in the archive.
  - **Removing an account** deletes its vectors immediately.
- **Being a good citizen.**
  - **Threads:** the indexer and ONNX Runtime's threads run at utility QoS.
  - **Pace:** between batches it reads the Mac's state and works 80% of the time on AC, 25% when the Mac reports "fair" thermal state, and 20% on battery. It pauses in Low Power Mode and at "serious" or "critical" thermal state.
  - **Queries first:** a query arriving mid-batch interrupts the batch (ONNX Runtime `RunOptions::terminate`, checked between graph nodes), runs, and the batch is retried. Background runs also never take the model while a query is waiting for it: the lock around the ONNX session isn't fair, and without that an indexer running piece after piece could take it ahead of a waiting query again and again (a query waited up to 2.7 s on Apple silicon).
  - **Deferring to typing:** batches wait for 250 ms without a query before starting. After 10 s of continuous queries, a batch runs anyway in uninterruptible pieces of two passages, each letting waiting queries go first for up to 250 ms, so indexing can't be starved forever and a query waits for at most two passages.
  - **Ask's sentences count as a query.** Ask compares up to 96 sentences with the question (docs/ASK.md). Those are embedded with `embed_passages_now`, which interrupts indexing like a query does instead of queueing behind it ([Ask's sentences](#asks-sentences)).
  - **Tested:** with a query every 80 ms throughout indexing, query p50 was 53 ms against 39 ms idle (worst 380 ms) on the Linux VM, and p50 36 ms, worst 98 ms, on the Apple-silicon runner (worst 1.2–2.7 s before). The interrupted batches gave the same vectors up to the batching noise described under [Batching](#batching) (cosine ≥ 0.9997 on x86-64, ≥ 0.9977 on Apple silicon), and every vector stayed nearest its own passage.
  - **Memory:** the model is unloaded after 10 minutes with no search and no indexing. Each passage run shrinks ONNX Runtime's CPU arena when it ends (`memory.enable_memory_arena_shrinkage`), and freed heap is handed back to the OS after the model loads (`penguin_semantic::mem`), after it is dropped or unloaded, and after a sweep that embedded anything (`src-tauri/src/memory.rs`, logged). See [Memory](#memory) and docs/PERFORMANCE.md, "The app process".

### How search uses it

Hybrid ranking lives in penguin-core ([docs/SEARCH-RANKING.md](SEARCH-RANKING.md)). It reads one slot, `AppState::semantic() -> Option<penguin_core::SemanticHandles {embedder, index, progress}>`, which the indexer fills. `Store::search_hybrid` uses it, and so does Ask through `Store::ask_with`.

- **Empty when off or not ready.** The slot is empty while the setting is off, and on first run until the model files have downloaded and verified. Search is then keyword-only.
- **The embedder is lazy.** The slot's embedder is the indexer itself. While the model is loaded it embeds. After 10 idle minutes the model is dropped, and the next query gets an immediate `ModelUnavailable`: that keystroke searches by keywords, and the model starts loading for the next one. A search never waits for a load.
- **Progress, and when meaning search starts.**
  - Until the first full pass over the mailbox completes, `progress` is messages embedded / messages to embed.
  - Below 1.0, hybrid search and Ask already use the partial index (`SemanticHandles::status` reports "indexing"). Mail is embedded newest first, so the part most searches are after is covered first, and mail not embedded yet is still found by its words. Waiting for the full pass would have meant keyword-only search for hours to a day on a large mailbox.
  - After the first completed pass, progress is 1.0 for good (a flag in semantic.db). New mail is embedded within seconds of arriving and doesn't send search back to keywords.
  - A model change starts over.
- **Filters apply before the cut.** The index is `SemanticIndex`. Hybrid calls `VectorIndex::search_where(query, k, &SearchFilter {account_ids, after, before, messages})`, and the index applies accounts, dates and the allow-list of messages that pass the query's other filters inside its scan. The result is exact over the subset, not the global nearest minus the rest. `search_where` is an addition to the contract with a default implementation (the old closure path), so `FlatIndex` and other stand-ins didn't change.
- **One cut for indexing and passages.** Chunk numbers come from one cut: `chunk_message(subject, authored)` (search re-cuts it to show a result's passage), and `chunk_mail(&MailDoc)` (the indexer: the same chunks, plus sender and attachment names in chunk 0, and one chunk for bulk mail). A test checks that the two agree on every chunk.
- **Scores rank, they don't threshold.** EmbeddingGemma cosines for a clearly right answer are often only 0.4–0.6 (the examples above). Hybrid normalizes and fuses them with the keyword side and keeps exact matches authoritative (research/02 §4).

`semantic_status` → `SemanticIndexStatus {state: off|downloading|loading|indexing|paused|ready|error, enabled, indexed, total, chunks, model, modelId, modelLicense, downloadDone, downloadTotal, pausedReason, error, indexRamBytes, rate}`, mirrored in `src/lib/types.ts`. It is the model and indexing status for Settings and debug info; search results carry their own `semantic` / `semanticProgress`. The setting is `Settings.semanticSearch` (default on; a new install asks in the Welcome setup before downloading anything, docs/ONBOARDING.md). Settings → Search shows the switch and a one-line progress, and Copy Debug Info includes a line for it.

## Decisions and evidence

### 1. The model: EmbeddingGemma 300M, 4-bit, first 256 dimensions

**Criteria:**
- retrieval quality on standard benchmarks and on email-like text in English, Spanish and Tagalog;
- license;
- download and memory;
- speed on a CPU.

**Evidence, public benchmarks.** Retrieval nDCG@10, computed from the raw per-task results in [embeddings-benchmark/results](https://github.com/embeddings-benchmark/results).
- ENGv2-R = MTEB(eng, v2) retrieval, 10 tasks. MMTEB-R = MTEB(Multilingual, v2) retrieval, 18 tasks.
- "Natural" = MMTEB-R without the 4 reasoning/synthetic tasks (SpartQA, TempReasonL1, WinoGrande, LEMBPasskey), which distort some headline numbers.
- The Tagalog and Spanish columns are the per-language subsets.
- These figures run 0.3–0.7 below the official leaderboard because of how LEMBPasskey is averaged; per-task numbers match it.

| Model | Params (non-embedding) | License | ENGv2-R | MMTEB-R | Natural | Tagalog (Belebele) | Spanish (MLQA) |
|---|---|---|---:|---:|---:|---:|---:|
| **EmbeddingGemma-300m** | 308M (≈100M) | Gemma Terms | 55.7 | 62.2 | **70.5** | 78.1 | 74.6 |
| multilingual-e5-small | 118M (21.6M) | MIT | 46.4 | 50.3 | 58.8 | **82.9** | 70.2 |
| snowflake-arctic-embed-m-v2.0 | 305M (113M) | Apache-2.0 | **58.4** | 54.4 | 60.7 | 74.4 | 70.8 |
| granite-embedding-97m-multilingual-r2 | 97M (28M) | Apache-2.0 | 50.1 | 59.6 | 61.5 | 69.2 | 62.4 |
| granite-embedding-107m-multilingual | 107M | Apache-2.0 | 47.9 | 47.6 | 54.2 | 62.7 | 63.4 |
| bekko-embedding-v1-a25m (Jul 2026) | 123M (25M) | MIT | 49.7 | 57.1 | 63.5 | 82.1 | 70.2 |
| bge-m3 | 568M | MIT | – | 54.3 | 62.0 | 91.6 | 83.7 |
| nomic-embed-text-v2-moe | 475M | Apache-2.0 | 54.8 | 56.7 | 64.9 | 87.4 | 73.3 |
| paraphrase-multilingual-MiniLM-L12-v2 | 118M | Apache-2.0 | 35.9 | 36.2 | 42.1 | 28.4 | 71.5 |
| potion-multilingual-128M (static) | 128M | MIT | 24.1 | 37.2 | 40.8 | 57.1 | 40.3 |

Models left out:
- Jina v5 nano/small: CC-BY-NC weights.
- harrier-oss-v1-270m: the strongest small multilingual scores (MMTEB-R 65.9), but it is a Gemma 3 derivative labeled MIT, which the Gemma terms' definition of derivatives makes doubtful.
- Qwen3-Embedding-0.6B: twice the size.
- nomic-embed-text-v1.5, bge-small and arctic-embed-xs/s: English-only in practice.
- Static models (model2vec/potion, static-*): about half the retrieval quality. The static-similarity card itself says it is "not intended for retrieval".

**Evidence, email-like text.** Penguin's own evaluation (`tests/fixtures/email_eval.json`) has 52 queries over 106 fictional messages, 41 of them decoys that share a topic or vocabulary with a right answer (another renewal, another flight, a water bill next to the electricity bill). Queries are paraphrases (EN/ES/TL), cross-language (English query, Spanish or Tagalog mail) and exact identifiers. Measured with ONNX Runtime 1.30 on this box (`~/.cache` scripts, results reproduced in the table):

| Model (ONNX file) | dims | R@1 | R@5 | MRR@10 | EN R@1 | ES R@1 | TL R@1 | Cross-language R@1 | Exact ids R@1 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **EmbeddingGemma, 4-bit (model_q4)** | 768 | **94.2** | 99.0 | **96.2** | **92.3** | 100 | 83.3 | **100** | 100 |
| EmbeddingGemma, fp32 | 768 | 92.3 | 99.0 | 95.8 | 88.5 | 100 | 83.3 | 100 | 100 |
| EmbeddingGemma, int8 (model_quantized) | 768 | 92.3 | **100** | 95.8 | 88.5 | 100 | 83.3 | 100 | 100 |
| EmbeddingGemma, int8, first 256 dims | 256 | 90.4 | 99.0 | 93.8 | 88.5 | 100 | 66.7 | 100 | 100 |
| multilingual-e5-small, fp32 | 384 | 76.9 | 97.1 | 86.4 | 76.9 | 85.7 | 83.3 | 57.1 | 83.3 |
| multilingual-e5-small, int8 | 384 | 71.2 | 95.2 | 83.7 | 69.2 | 71.4 | 83.3 | 42.9 | 100 |
| bekko-embedding-v1-a25m | 384 | 76.9 | 93.3 | 84.7 | 69.2 | 100 | 66.7 | 85.7 | 83.3 |
| granite-embedding-107m-multilingual | 384 | 76.9 | 91.3 | 84.7 | 69.2 | 100 | 66.7 | 71.4 | 100 |
| potion-multilingual-128M (static) | 256 | 63.5 | 83.7 | 71.1 | 46.2 | 85.7 | 66.7 | 71.4 | 100 |
| granite-embedding-97m-multilingual-r2 | 384 | 57.7 | 74.0 | 66.4 | 61.5 | 71.4 | 66.7 | 14.3 | 66.7 |
| snowflake-arctic-embed-m-v2.0, int8 | 768 | 55.8 | 76.9 | 65.5 | 57.7 | 71.4 | 50.0 | 71.4 | 16.7 |
| paraphrase-multilingual-MiniLM-L12-v2, int8 | 384 | 46.2 | 79.8 | 61.7 | 65.4 | 57.1 | 0.0 | 28.6 | 16.7 |
| BM25 (keyword ranking, for reference) | – | 48.1 | 69.2 | 57.7 | – | 71.4 | 66.7 | 28.6 | 100 |

52 queries is a small set: one query is 1.9 points of R@1, and the per-language columns have 6–26 queries each. What it does show, consistently with the public benchmarks, is a wide gap between EmbeddingGemma and every smaller model on paraphrase and cross-language queries. The arctic int8 export scores far below its published numbers, so it is probably a quantization or export problem. It wasn't pursued, since the model is Gemma-sized anyway.

**Evidence, Penguin's search-eval harness.** [docs/SEARCH-EVAL.md](SEARCH-EVAL.md) describes the harness: a 19,183-message synthetic mailbox and 200 judged queries.
- **Scores:** nDCG@10 per category.
- **Hybrid:** the harness's reference fusion, reciprocal rank fusion (k = 60) of `Store::search` and the vector list, with operators kept as hard filters. It is not the final ranking, which another change builds.
- **Chunking:** the harness's own chunker and exact f32 search.
- **Commands:** `search-eval run --mode vector|hybrid --embedder gemma|e5-small`, then `search-eval diff`. Measured 2026-09-26.

| category (n) | keyword | vector, e5-small | vector, Gemma | hybrid, e5-small | **hybrid, Gemma** |
|---|---:|---:|---:|---:|---:|
| identifier (23) | 1.000 | 0.142 | 0.410 | 0.993 | 0.958 |
| misspelling (17) | 0.059 | 0.807 | 0.890 | 0.807 | 0.890 |
| name (17) | 0.981 | 0.661 | 0.865 | 0.948 | 0.959 |
| natural (24) | 0.921 | 0.940 | 0.944 | 0.979 | 0.985 |
| operator (26) | 0.959 | 0.297 | 0.541 | 0.936 | 0.937 |
| paraphrase (33) | 0.000 | 0.581 | 0.825 | 0.581 | **0.806** |
| question (28) | 0.107 | 0.780 | 0.860 | 0.780 | 0.860 |
| spanish (18) | 0.346 | 0.806 | 0.797 | 0.813 | 0.814 |
| time (14) | 0.286 | 0.192 | 0.393 | 0.473 | 0.603 |
| **all (200)** | 0.505 | 0.584 | 0.736 | 0.814 | **0.876** |
| queries with no result | 93 | 8 | 8 | 0 | 0 |

The paired randomization test in `search-eval diff` gives:
- **Gemma hybrid vs keyword:** +0.372 overall (p < 0.001; 96 queries better, 11 worse), significant on paraphrase, question, Spanish, misspelling and time.
- **Gemma hybrid vs e5-small hybrid:** +0.062 overall (p < 0.001) and +0.225 on paraphrase (p < 0.001, 15 wins, 0 losses).
- **What plain fusion costs:** identifiers drop from 1.000 to 0.958 and names from 0.981 to 0.959 (neither significant). A vector list can push an exact match to second place. That is the ranking's job to prevent (exact matches authoritative, research/02 §4), not the model's.

On cross-language queries (7 in this set) e5-small is slightly ahead (0.617 vs 0.555), consistent with its strong Tagalog and Spanish benchmark columns.

Query latency in these runs (p50 77 ms for Gemma hybrid, 48 ms for e5-small) is dominated by the query embedding on this shared, busy x86 VM. Apple-silicon numbers are listed under "Not verified".

**The product path.** `search-eval run --mode penguin --embedder gemma` runs what the app runs: `Store::search_hybrid` (routing, fusion, filters on both sides, soft dates and respelling from docs/SEARCH-RANKING.md) over vectors cut by the app's own chunker (`chunk_mail`), measured 2026-09-27 against a fresh keyword run.

| category (n) | keyword | penguin, Gemma, earlier 800-char cut | **penguin, Gemma** |
|---|---:|---:|---:|
| identifier (23) | 1.000 | 0.997 | 0.997 |
| misspelling (17) | 0.059 | 0.911 | 0.872 |
| name (17) | 0.981 | 0.943 | 0.950 |
| natural (24) | 0.921 | 0.981 | 0.987 |
| operator (26) | 0.959 | 0.965 | 0.967 |
| paraphrase (33) | 0.000 | 0.810 | 0.827 |
| question (28) | 0.107 | 0.855 | 0.885 |
| spanish (18) | 0.346 | 0.709 | 0.758 |
| time (14) | 0.286 | 0.659 | 0.735 |
| **all (200)** | 0.505 | 0.879 | **0.894** |

- **Against keyword search:** +0.389 (p < 0.001; 94 queries better, 2 worse). Hybrid search's own measurements with bge-small and MiniLM gave 0.811 and 0.810.
- **Choosing the chunker.** The middle column is the character-based cut hybrid search started with: ~800 characters ending at a sentence, no overlap, no sender, no cleanup. The unified cut is ahead by +0.015 overall (26 wins, 10 losses). That is not significant (p = 0.10), and misspellings lose 0.04 while Spanish and time gain. It was kept for that direction and for what it adds (the sender and attachment names, boilerplate removed), and there is now one chunker.
- **The one query below the bar.** Identifiers, operators and natural queries stay at or above 0.96. Names are at 0.950 because of one query: `dr patel` drops from 1.00 to 0.39. Its target, the dental reminder, is the only keyword match, so hybrid fuses it as a balanced query, and meaning ranks a doctor's results email and threads with a colleague named Patel above it. That is routing (a name whose words all match should stay keyword-led), handed back to hybrid ranking.

**Why EmbeddingGemma.** It is the best retrieval model at its size on every measure that matters here:
- **Public benchmarks:** best "natural" multilingual retrieval (70.5 vs 58.8 for e5-small), and strong English (55.7 vs 46.4) and Spanish.
- **Email eval:** best on paraphrase and cross-language queries.
- **Tagalog:** good (78.1 on Belebele; e5-small's 82.9 is the one column it doesn't lead).
- **Built for this:** Google designed it for on-device search, and its card documents Matryoshka output sizes and quantization-aware checkpoints ([model card](https://ai.google.dev/gemma/docs/embeddinggemma/model_card), [announcement](https://developers.googleblog.com/en/introducing-embeddinggemma/)). Its retrieval prompts are used as documented: `task: search result | query: ` for queries and `title: none | text: ` for passages.

**Why the 4-bit export.** Of the three ONNX files in [onnx-community/embeddinggemma-300m-ONNX](https://huggingface.co/onnx-community/embeddinggemma-300m-ONNX):
- **int8 (`model_quantized`) is unusable on CPU.** It stores weight-only QDQ (`DequantizeLinear` → `MatMul`/`Gather`), so ONNX Runtime dequantizes the 262,144 × 768 embedding table on every call: about 600 ms per query here, measured.
- **4-bit (`model_q4`) uses the right kernels.** It uses `MatMulNBits` and `GatherBlockQuantized`, ONNX Runtime's kernels for block-quantized weights.
- **Quality:** it scored as well as fp32 on the email set (R@1 94.2 vs 92.3; the difference is one query).
- **Size:** it is a 197 MB download (plus a 20 MB tokenizer) instead of 1.2 GB.
- **Not fp16:** the card warns that EmbeddingGemma activations don't support fp16.
- **Google's own quantization results:** its quantization-aware Q4_0 checkpoint loses 0.5 points on MMTEB (61.15 → 60.62), per the model card.

**Why 256 dimensions.** EmbeddingGemma is Matryoshka-trained. The [model card](https://ai.google.dev/gemma/docs/embeddinggemma/model_card) reports mean scores over all tasks, not retrieval alone, at each output size:
- MTEB(English, v2): 69.67 at 768 dims, 69.18 at 512, 68.37 at 256.
- MTEB(Multilingual, v2): 61.15, 60.71 and 59.68.

That is about −1.3 to −1.5 points for a third of the storage and scan cost. At 600k chunks that is 154 MB of int8 on disk instead of 461 MB, and 89 MB of int4 scan data in RAM instead of 240 MB. On the email set, 256 dims cost two queries of R@1 (90.4 vs 92.3). The recall of the quantized index at 256 dims is under [Measurements](#measurements).

**The trade-offs, stated plainly:**
- **License.** EmbeddingGemma is under the [Gemma Terms of Use](https://ai.google.dev/gemma/terms) and [Prohibited Use Policy](https://ai.google.dev/gemma/prohibited_use_policy), not an OSI license. Commercial use and redistribution are allowed with the notice and use restrictions passed on. Penguin doesn't redistribute the weights: the app downloads them from Hugging Face on the user's Mac. Settings shows the model name and license, and the upstream repo `google/embeddinggemma-300m` is gated, while the onnx-community mirror Penguin downloads from is not. If a fully permissive model is ever required, `model::E5_SMALL_INT8` (MIT) is already specified. Pointing `model::DEFAULT` at it re-embeds everything on the next start, at a real cost in quality (the tables above).
- **Compute.** EmbeddingGemma has about 4–5× the per-token compute of e5-small (≈100M vs 21.6M non-embedding parameters). The backfill takes correspondingly longer; the measured rates are below. Queries stay fast because a query is ~10–20 tokens.
- **Apple's built-in NLContextualEmbedding** was considered and rejected:
  - It would avoid a download, but it outputs token-level vectors and Apple documents it for training classifiers and taggers, pointing to `NLEmbedding` for similarity. It is not a retrieval model.
  - It covers 27 languages; whether Tagalog is among them is unverified.
  - It is Swift- and macOS-only, so it can't be tested in CI on Linux.
  - Source: [NLContextualEmbedding](https://developer.apple.com/documentation/naturallanguage/nlcontextualembedding) and WWDC23 session 10042.

### 2. The runtime: ONNX Runtime through `ort` 2.0.0-rc.13, statically linked, CPU

| Option | For | Against |
|---|---|---|
| **ort** (ONNX Runtime 1.28) | Fastest measured CPU path for BERT-class encoders (3,052 vs 603 embeddings/s for candle on an M4 Max in the one public side-by-side, [rust-embedding-bench](https://github.com/jerrythomas/rust-embedding-bench)); runs EmbeddingGemma's ONNX export as published; the same code on macOS arm64 and Linux x86-64 (CI, this box); **links statically on macOS**, so no dylib to sign or notarize ([maintainer](https://github.com/pykeio/ort/issues/388), [linking docs](https://ort.pyke.io/setup/linking)); MatMulNBits / GatherBlockQuantized kernels for 4-bit weights; `RunOptions::terminate` for preemption | Still a release candidate (pinned exactly); the build downloads the prebuilt library from pyke's CDN (sha256-pinned in ort-sys); adds roughly 30–40 MB to the binary (estimate from the 39 MB macOS dylib; the static archive is 80 MB before linking) |
| candle 0.11 | Pure Rust, Metal | Its `gemma3` is the causal decoder, not EmbeddingGemma's bidirectional encoder with pooling and dense heads; no quantized encoders; plain-CPU speed about 5× below ORT in the benchmark above |
| tract 0.23 | Pure Rust | No published BERT-class speed comparison; ONNX-contrib ops coverage for this graph (MatMulNBits, GatherBlockQuantized) unverified |
| llama.cpp (`llama-cpp-2`) | Metal; fastest batch throughput measured (948 vs 261 embeddings/s at 256 tokens, same benchmark) | C++/CMake build; GGUF conversion; no gain at batch 1 (queries). The `Embedder` trait keeps it open as a later backfill accelerator |

GPU and Neural Engine execution providers were measured, not assumed: Core ML is 2–3× slower on this graph and isn't used; WebGPU works and is an opt-in build feature ([section 6](#6-accelerators-webgpu-yes-opt-in-core-ml-no)). The tokenizer is Hugging Face `tokenizers` 0.23 built with `fancy-regex` instead of Oniguruma, so there is no C code.

The release build is aarch64-only (`.github/workflows/release.yml`), and ort's prebuilt `aarch64-apple-darwin` library is what it links. ONNX Runtime no longer ships x86-64 macOS builds ([ort#556](https://github.com/pykeio/ort/issues/556)), which doesn't matter here.

### 3. Shipping the weights: downloaded once, pinned, verified

- **Not bundled.** Every push to main makes a full `.app.tar.gz` for the updater, so ~217 MB of weights in the bundle would be re-downloaded on every update.
- **Downloaded once.** `huggingface.co/onnx-community/embeddinggemma-300m-ONNX/resolve/5090578d…/<file>` (a full commit sha; the resolver redirects LFS files to Hugging Face's CDN).
- **Verified.** Each file is written to `<file>.part` while being hashed, and renamed only if its size and sha256 match `model.rs`. The sha256 is Hugging Face's LFS object id, checked against the downloaded bytes on this box. An interrupted download resumes with a Range request. At each launch the files are checked again, but hashed only if they changed since they last passed (`model::verify_cached`: a stamp of size, inode, and modification and change times). Hashing all 217 MB at every launch cost 1.5 s of CPU on the build VM (docs/PERFORMANCE.md, "The app process").
- **Rate limits and privacy.** Anonymous Hugging Face downloads allow 3,000 resolver requests per 5 minutes per IP ([rate limits](https://huggingface.co/docs/hub/rate-limits)), far more than three files need. No account, token or cookie is sent.
- **Never blocking.** Everything runs on the indexer thread. Until the model is there, search is keyword-only and Settings shows download progress.

### 4. The index: exact filters, int4 scan, int8 rescoring, all in SQLite + RAM

Options weighed for ~600k chunks:
- **[sqlite-vec](https://github.com/asg017/sqlite-vec)** is simple, but its flat scan is slow. The author measured 590 ms per query at 1M × 1024 f32 on an M4, and the Rust crate compiles its C without the NEON kernels. Its ANN modes (DiskANN, IVF) are alpha: DiskANN took 55 minutes to insert 1M vectors (PRs [#276](https://github.com/asg017/sqlite-vec/pull/276), [#278](https://github.com/asg017/sqlite-vec/pull/278)).
- **[USearch](https://github.com/unum-cloud/usearch)** (HNSW) is fast, but it is a C++ build, and a graph index is overkill at this size: rebuilds, deletions and filtered search all add complexity.
- **A brute-force scan in Rust** is what Penguin uses. At 256 dims the int4 codes of 600k chunks are 77 MB, and the scan is a single linear pass. Filters are exact and deletions are trivial.

Quantization evidence:
- Hugging Face's [embedding quantization study](https://huggingface.co/blog/embedding-quantization): binary vectors keep about 92.5% of retrieval quality, and up to ~96% when the top candidates are rescored. int8 with rescoring keeps about 99%. It varies strongly by model: binary kept 96.45% for mxbai-embed-large-v1 (1024 dims) but only 74.77% for e5-base-v2.
- **Measured on EmbeddingGemma itself.** The first version of the index used sign bits and was replaced after measuring it. On 6,000 real EmbeddingGemma vectors of Enron email (the public [SetFit/enron_spam](https://huggingface.co/datasets/SetFit/enron_spam) text, 60-word chunks) with 284 queries, Recall@10 against exact float search was:

  | dims | int8, exhaustive | binary → int8, pool 100 / 400 | int4 → int8, pool 50 / 100 / 200 |
  |---:|---:|---:|---:|
  | 768 | 0.992 | 0.950 / 0.989 | – |
  | 512 | 0.990 | 0.908 / 0.975 | 0.986 / 0.986 / 0.985 |
  | 256 | 0.986 | 0.799 / 0.931 | 0.988 / 0.987 / 0.987 |
  | 128 | 0.975 | 0.640 / 0.853 | – |

  (int4 columns: 200 queries.) Centering the vectors before taking signs helped binary a little at 256 dims (0.843 / 0.950) but not enough. Tiled to 60,000 vectors (each copied with small noise), int4 → int8 at pool 100 still recalls 0.990 at 256 dims. The binary pool would have to grow with the corpus, while int4 matches exhaustive int8 at half its RAM. So: int4 in RAM for the pass, and int8 in semantic.db for rescoring the pool.
- Matryoshka truncation: [Hugging Face, Matryoshka embeddings](https://huggingface.co/blog/matryoshka).
- For this model the recall is measured, not assumed (below).

### 5. Embedding speed: batching, threads, queries

Every number in sections 5–8 was measured with `examples/embed_perf.rs` (`PERF_MODE=sweep|tokenizer|cycles|padding`, one process per configuration) and the search-eval harness, before and after each change, on two machines:

- **Linux:** the shared build VM (Intel Haswell, 8 vCPUs), 4 threads. Other agents' builds pushed the load average to 5–11 during these runs, so passages per second swing from pass to pass; CPU seconds per passage is the steadier measure. Rows are only compared with rows measured in the same interleaved pass.
- **Apple silicon:** GitHub's macOS runner, 3 cores of a virtual M1 with 7.5 GB and a paravirtualized GPU, macOS 26, 3 threads (`.github/workflows/macos-embed.yml`; runs [36294897364](https://github.com/justingluska/penguin/actions/runs/36294897364), [36297014601](https://github.com/justingluska/penguin/actions/runs/36297014601), [36299323975](https://github.com/justingluska/penguin/actions/runs/36299323975); experiments [36296378224](https://github.com/justingluska/penguin/actions/runs/36296378224), [36299349524](https://github.com/justingluska/penguin/actions/runs/36299349524), [36300676460](https://github.com/justingluska/penguin/actions/runs/36300676460)). A real Mac has more and faster cores. The runner is the nearest thing to one this project can measure on, and it is a shared VM too.

The passages are the eval harness's mailbox cut by `chunk_mail` (`search-eval texts`: 16,378 passages, 56.8 tokens on average with the prompt, p99 105, longest 136), embedded in calls of 30 like the app's indexer (16 messages at a time). The queries are the harness's 185 query texts.

#### Batching

- **Before.** Passages were sorted by characters and run 16 at a time, padded to the longest. That computed 1.30 tokens for every real one: on a CPU, a padding position costs as much as a real token.
- **Now.** Passages are tokenized first, sorted by token count, and packed into runs of at most **256 padded tokens** (and at most 64 passages). Padding falls to 1.04. Sorting by length before batching is what sentence-transformers does in `encode` ([the sort](https://github.com/huggingface/sentence-transformers/blob/v6.1.0/sentence_transformers/sentence_transformer/model.py#L925-L928)), "so that padding waste is minimised within each batch" ([the reason](https://github.com/huggingface/sentence-transformers/blob/v6.1.0/sentence_transformers/base/model.py#L505-L510)). A token budget instead of a passage count means short passages go in wide runs and long ones in narrow runs, so a run's activations, and the ONNX Runtime arena that holds them, stay small.
- **Measured.** Three interleaved passes each, passages per second and CPU seconds per passage:

  | batching | Linux, 4 threads: passages/s | CPU s / passage | Mac runner, 3 threads: passages/s (6 passes) | CPU s / passage |
  |---|---:|---:|---:|---:|
  | before: 16 per run | 5.6 / 5.0 / 7.1 | 0.42 / 0.45 / 0.41 | 13.5–25.2, median 22.3 | 0.09–0.14 |
  | one passage per run | 6.0 / 4.9 / 8.3 | 0.33 / 0.38 / 0.33 | 15.3–21.9, median 20.3 | 0.09–0.13 |
  | 128 tokens | 7.0 / 5.9 / 9.1 | 0.32 / 0.32 / 0.32 | 12.4–22.8, median 19.2 | 0.09–0.15 |
  | **256 tokens** | **7.2 / 4.9 / 10.0** | **0.32 / 0.37 / 0.31** | 15.8–25.1, median 22.1 | 0.08–0.13 |
  | 512 tokens | 5.8 / 5.9 / 8.0 | 0.39 / 0.37 / 0.40 | 17.5–27.6, median 24.5 | 0.08–0.11 |

  On Linux the 256-token budget used 20–25% less CPU per passage than the old batching. On the Mac runner every setting falls inside the runner's noise in this benchmark. Indexing the whole harness mailbox (19,183 passages, calls of 256 messages) on the runner took 1,995 CPU-seconds with the old batching, 1,355 with 256-token runs and 1,296 with 512 ([36299349524](https://github.com/justingluska/penguin/actions/runs/36299349524)): about a third less. 512 was not clearly better than 256 on the Mac and was worse on Linux, and 256 keeps runs smaller for the reasons below.
- **Vectors depend a little on what shares a run.** `PERF_MODE=padding` embeds 96 passages alone and batched, and compares each with itself alone:

  | batching | padded / real | Linux: min / mean cosine | Apple silicon | Apple silicon, KleidiAI off |
  |---|---:|---:|---:|---:|
  | alone, a second time | 1.00 | 0.999999 / 1.000000 | 0.999999 / 1.000000 | 0.999999 / 1.000000 |
  | 256-token runs (now) | 1.02 | 0.999999 / 1.000000 | 0.997937 / 0.999732 | – |
  | 512-token runs | 1.04 | 0.999719 / 0.999990 | 0.997856 / 0.999327 | 0.999613 / 0.999819 |
  | pieces of 2 | 1.01 | 0.999998 / 1.000000 | 0.998128 / 0.999878 | 0.999613 / 0.999820 |
  | 16 per run (before) | 1.11 | 0.999719 / 0.999967 | 0.997685 / 0.998891 | 0.999602 / 0.999805 |
  | one wide batch | 1.75 | 0.999666 / 0.999848 | 0.997685 / 0.998371 | 0.999661 / 0.999781 |
  | the same passage twice in one run | 1.00 | 0.999999 / 1.000000 | 0.999999 / 1.000000 | 0.999613 / 0.999820 |

  On x86-64 the differences are float noise. On Apple silicon they come from ONNX Runtime's KleidiAI kernels, the Arm int8 path for the model's 4-bit `MatMulNBits` layers (`accuracy_level` 4 means "input A can be quantized with the same block_size to int8 internally", [MatMulNBits](https://github.com/microsoft/onnxruntime/blob/v1.28.2/docs/ContribOperators.md#commicrosoftmatmulnbits); KleidiAI was integrated in ONNX Runtime 1.22, [release notes](https://github.com/microsoft/onnxruntime/releases)). Switching them off with the session option `mlas.disable_kleidiai` brings the Mac in line with Linux, but costs 27% of passage throughput (18.7 → 13.6 passages/s) and makes queries 18% slower (22.0 → 25.9 ms p50), so they stay on. With KleidiAI a vector depends on the other passages in its run, not on padding as such (the same passage twice agrees exactly), and the mean difference grows with the run's size. Small runs keep passages closest to how queries are embedded (alone).
  - **Effect on search quality, Apple silicon** (search-eval, CPU provider, same day): old batching nDCG@10 0.901, 256-token runs 0.898, 512 0.896, one passage per run 0.896; none of the differences is significant (p ≥ 0.17; 7 wins and 7 losses for 256). Identifier, operator and natural stay at 0.997, 0.967 and 0.987 in all of them. The Linux base run is 0.894: the platform matters more than the batching.
- **Queries during indexing.** A stop-and-retry batch that has to fall back to uninterruptible pieces now yields to waiting queries at the session lock ([How it works](#how-it-works)). `tests/real_model.rs` types a query every 80 ms throughout indexing: on the Mac runner the worst query went from 1.2–2.7 s (three runs) to 98 ms (p50 36 ms). On Linux it was 313 ms before and 262–380 ms after (p50 54 → 49–53 ms): no change there. The price is that indexing moves more slowly under a nonstop stream of queries (the test's 106 messages took 98 s before and 214 s after on Linux), which is the intended order: the person typing comes first.

#### Sequence length

- EmbeddingGemma reads up to 2,048 tokens ([model card](https://huggingface.co/google/embeddinggemma-300m)). Penguin caps at 256 (`ModelSpec::max_tokens`, part of the model id), and the chunker's 120-word windows run about 160–240 tokens, so the cap rarely cuts: in the harness mailbox no passage exceeds 136 tokens.
- At these lengths the cost is almost linear in tokens. Per token and layer the linear layers take about 4.2 M multiply-adds (hidden size 768, 3 query heads and 1 key/value head of 256, a gated MLP of 1,152: the model's [config.json](https://huggingface.co/onnx-community/embeddinggemma-300m-ONNX/blob/5090578d9565bb06545b4552f76e6bc2c93e4a66/config.json)); attention over n tokens adds 1,536 × n, which is 9% at 256 tokens and 2% at 57. A lower cap would only save the tokens it cuts off, which is the text. It stays at 256.

#### Threads

- ONNX Runtime gets half the cores, 1–4 intra-op threads (its default is every physical core, [threading](https://onnxruntime.ai/docs/performance/tune-performance/threading.html)), at utility QoS. On the Mac runner 1, 2 and 3 threads gave 14.7, 23.9 and 28.0 passages/s at 0.07–0.08 CPU seconds per passage: near-linear, with no waste from the extra threads.
- **Spinning stays off.** ONNX Runtime's workers can spin while waiting for work, which "provides faster inference but consumes more CPU cycles, resources, and power" ([threading](https://onnxruntime.ai/docs/performance/tune-performance/threading.html)); its non-client builds default to it ([config keys](https://github.com/microsoft/onnxruntime/blob/v1.28.2/include/onnxruntime/core/session/onnxruntime_session_options_config_keys.h#L174-L213)), so Penguin sets it explicitly. On the Mac runner spinning cut query p50 from ~17 to 9–16 ms and raised passages/s by 10–20%, at similar CPU per passage. On the busy Linux VM it raised CPU per query from 94–105 to 152–196 ms and CPU per passage by 40%: spinning threads compete with whatever else the machine runs. It is a property of the session's thread pool, and one session is shared, so it can't be on for queries only. Off, until energy is measured on a real Mac (below).

#### Queries

- **Latency.** Linux, 4 threads: p50 38–40 ms, p95 49 ms (before: 42 / 57 ms). Mac runner, 3 threads: p50 16.5–19.7 ms, p95 19–27 ms.
- **The query cache** (64 recent texts, hybrid.rs) now serves Ask too. Ask embedded its question twice, once to retrieve and once to score sentences; it now embeds it once.

#### Ask's sentences

Ask compares up to 96 sentences with the question. Before, those went through `embed_passages`, the indexer's path: they waited a quarter second for quiet after the question's own embedding, and queued behind indexing runs. `embed_passages_now` treats them like a query. Time to embed the 96 sentences after the question (Mac runner, 3 threads, 256-token runs, three passes):

| | idle | while the indexer runs |
|---|---:|---:|
| indexer's path (before) | 2.3–3.9 s | 2.6–7.7 s |
| foreground path (now) | 1.9–3.6 s | 2.4–2.8 s |

On Linux (busy VM) the same was 6.7–30.3 s before and 6.4–13.4 s now while indexing. This is still the largest embedding latency in the app. Caching sentence vectors or embedding fewer sentences would cut it, but changes what Ask reads and needs its own evaluation (docs/ASK.md).

### 6. Accelerators: WebGPU yes (opt-in), Core ML no

The ONNX graph uses Microsoft contrib operators: 170 `MatMulNBits` (4-bit, block 32, `accuracy_level` 4), `GatherBlockQuantized` for the embedding table, 24 `MultiHeadAttention`, `RotaryEmbedding` and `SimplifiedLayerNormalization`, with dynamic batch and sequence dimensions. That decides which accelerators can take it.

- **Core ML: measured, not used.** ONNX Runtime's Core ML provider has no builder for any of those operators ([op_builder_factory.cc](https://github.com/microsoft/onnxruntime/blob/v1.28.2/onnxruntime/core/providers/coreml/builders/op_builder_factory.cc#L15-L99)), so it can take only the glue between them. Its documentation also warns that dynamic shapes may hurt performance and that without a cache directory it recompiles the model on every load ([Core ML EP](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html)). On the Mac runner (MLProgram, all four compute-unit settings, run [36294897364](https://github.com/justingluska/penguin/actions/runs/36294897364)) Core ML rejected its pieces ("has unbounded dimension which is not supported"), fell back partition by partition, and ran 9.9–13.7 passages/s against the CPU's 28.4, with query p50 49–63 ms against 17.5, 9–11 s to load against 0.8 s, and 1.7 GB peak memory against 0.5 GB. Penguin doesn't use it.
- **The Neural Engine** needs more than a provider switch. Apple's guidance for transformers on the ANE is a rewritten model: 4-D channels-first tensors, convolutions instead of linear layers, attention split per head ([Deploying Transformers on the Apple Neural Engine](https://machinelearning.apple.com/research/neural-engine-transformers)), with fixed or enumerated input shapes for the best performance ([coremltools, flexible inputs](https://apple.github.io/coremltools/docs-guides/source/flexible-inputs.html)). That is a different model file, converted and validated separately; the runner has no ANE to measure one on.
- **WebGPU: works, opt-in.** ONNX Runtime has WebGPU kernels for every operator in this graph ([webgpu_contrib_kernels.cc](https://github.com/microsoft/onnxruntime/blob/v1.28.2/onnxruntime/contrib_ops/webgpu/webgpu_contrib_kernels.cc#L23-L45)), and natively runs them on Dawn, which uses Metal on macOS ([WebGPU EP](https://onnxruntime.ai/docs/execution-providers/WebGPU-ExecutionProvider.html)). On the Mac runner's paravirtualized GPU:

  | | CPU, 3 threads | WebGPU |
  |---|---:|---:|
  | passages/s | 27.7–28.4 | 30.6–32.3 |
  | CPU seconds per passage | 0.08 | 0.00–0.01 |
  | query p50 | 16.6–17.6 ms | 28–30 ms |
  | Ask's 96 sentences (idle) | 2.3 s | 1.6–1.7 s |
  | resident after load | 511–517 MB | 199–210 MB |
  | agreement with the CPU's vectors | – | cosine ≥ 0.9983 |

  Wider runs didn't help (1,024 tokens: 31.4 passages/s; 4,096: 23.1). The GPU takes indexing off the CPU almost entirely; queries are faster on the CPU, where a 20-token input doesn't cover the cost of launching GPU work for each of the graph's ~1,600 nodes. It isn't on by default because ort's WebGPU build loads Dawn as a dylib (`libwebgpu_dawn.dylib`, 8.7 MB) that release builds would have to bundle, sign and notarize, and because a real Mac's GPU speed, energy use, and any contention with the WebKit UI are unmeasured. `--features embed-webgpu` builds the app with it for trying on a Mac (Dawn's dylib must be where the binary can load it, e.g. `target/debug/deps` under `tauri dev`); the embedder falls back to the CPU if WebGPU can't start and logs which provider runs. A shipping version would most likely use the GPU for indexing and keep queries on the CPU, at the cost of a second session's memory.
- **IOBinding and graph capture** don't apply yet. IOBinding keeps inputs and outputs on the device ([IOBinding](https://onnxruntime.ai/docs/performance/tune-performance/iobinding.html)); here they are two small integer tensors in and a few KB out. WebGPU graph capture needs static shapes ([WebGPU EP](https://onnxruntime.ai/docs/execution-providers/WebGPU-ExecutionProvider.html)), which variable-length passages don't have.

### 7. Memory

- **Tokenizer.** Gemma's `tokenizer.json` is 20 MB: a BPE model with 262,144 tokens and 514,906 merges. `Tokenizer::from_file` reads the whole file into a string and parses it with serde ([source](https://github.com/huggingface/tokenizers/blob/v0.23.2/tokenizers/src/tokenizer/mod.rs#L468-L471)), and the allocator keeps the parse's garbage. On Linux that is +289 MB resident after loading, of which `malloc_trim` hands back 193 MB, leaving 96 MB ("since glibc 2.8 this function frees memory in all arenas and in all chunks with whole free pages", [malloc_trim(3)](https://man7.org/linux/man-pages/man3/malloc_trim.3.html)). `mem::release_freed` does this after every load and every unload. On macOS the runner showed 398 MB resident but a 144 MB physical footprint, the number Activity Monitor shows, and `malloc_zone_pressure_relief` ([malloc.h](https://github.com/apple-oss-distributions/libmalloc/blob/main/include/malloc/malloc.h#L516-L522)) changed neither: the freed pages already count as reusable there.
- **A leak in the tokenizer's word cache.** tokenizers 0.23 keeps BPE's word cache in thread-locals keyed by tokenizer instance, up to 10,000 words per thread ([bpe/model.rs](https://github.com/huggingface/tokenizers/blob/v0.23.2/tokenizers/src/models/bpe/model.rs#L28-L90), [cache.rs](https://github.com/huggingface/tokenizers/blob/v0.23.2/tokenizers/src/utils/cache.rs#L6-L10)), and nothing removes an instance's entries when it is dropped. The app's indexer thread lives for the whole session and reloads the model after every idle unload, so each reload left another set behind. `PERF_MODE=cycles` (load, tokenize the mailbox, drop, five times on one thread) measured +40 MB per cycle on Linux and +48 MB of footprint per cycle on the Mac runner (193, 211, 287, 335, 383 MB). With the cache off (`resize_cache(0)`) it stays flat (164, 135, 156, 163, 172 MB). One earlier Mac run grew with the cache off too (158 → 388 MB); two later runs didn't, so that is noted for a real Mac. Tokenizing without the cache is up to 45% slower on Linux (13,500 → 7,400–9,500 passages/s) and within noise on the Mac runner: at most 0.1 ms a passage, against 35–220 ms to embed one.
- **ONNX Runtime's arena.** Each session's CPU arena grows to the largest run and "the memory allocated by the arena is never returned to the system" ([C API guide](https://onnxruntime.ai/docs/get-started/with-c.html)). Smaller runs keep it smaller, and dropping the session after 10 idle minutes (unchanged) frees it.
- **Totals** (bench.rs on Linux, two interleaved passes): resident growth after loading 414–415 → 219–226 MB, after indexing 471–520 → 291–297 MB. The eval harness's whole run (Linux, search-eval `--mode penguin --embedder gemma`) peaked at 614 MB resident before and 511 MB after.

### 8. Low-value mail

Most mail is sent by machines: "more than 60% of the overall email traffic" in 12.5 million Yahoo inboxes ([Ailon et al., WSDM 2013](https://dl.acm.org/doi/10.1145/2433396.2433447)), and "non-spam machine-generated messages actually represent 90% of the entire dataset" in six months of Yahoo mail ([Grbovic et al., CIKM 2014](https://arxiv.org/abs/1606.09296)). So what the indexer does with it decides most of the backfill.

- **The policy.** Spam and drafts are never embedded. Bulk mail (a List-Unsubscribe header, or Gmail's Promotions, Updates or Forums category) gets one chunk: subject, sender, attachment names and opening. Personal mail gets up to eight.
- **Measured: what the one chunk is worth.** The harness mailbox has 3,511 bulk messages out of 19,183 (18%). Leaving them out of the vector index entirely (`EVAL_SEMANTIC_BULK=skip`; keyword search still finds them) saves 18% of the chunks and costs nDCG@10 0.893 → 0.835 (p < 0.001; 5 queries better, 27 worse). Paraphrase falls 0.827 → 0.647, misspelling 0.872 → 0.753, question 0.885 → 0.824: receipts, bookings and notices are bulk by these rules, and people search for them by what they were about.
- **Decision.** Keep one chunk for bulk mail. A cheaper policy would give up relevance, which this round doesn't trade. What would make a large archive useful sooner without losing anything is ordering, not skipping: embedding a page's personal mail before its bulk mail. That changes the sweep's paging (cursor, deletions, resume) and isn't done here.

## Measurements

All numbers below come from `cargo run -p penguin-semantic --release --features onnx --example bench`. Sections 5–8 have the embedding-speed and memory measurements, including Apple silicon.
- **Machine:** the shared Linux build VM (Intel Haswell, 8 vCPUs, other builds running; 1-minute load average 4–9).
- **Index section:** real EmbeddingGemma vectors (`PENGUIN_SEMANTIC_VECTORS`: 6,000 Enron passages) tiled with small noise to 600,000 chunks, the size of a 300k-message mailbox at two chunks a message. 256 dimensions.
- **Model section:** the pinned 4-bit model (`PENGUIN_SEMANTIC_MODEL_DIR`, `BENCH_ONLY=model`).
- **Apple silicon:** the index numbers were not measured on a Mac; the model's are in sections 5–7 (GitHub's M1 runner). See [Not verified on a Mac yet](#not-verified-on-a-mac-yet).

### Vector index, 600,000 chunks

| | |
|---|---:|
| **Search, all mail** (`search_messages`, k = 50) | **p50 17.0 ms, p95 24.8 ms** |
| Search, one account (10% of mail) | p50 9.1 ms, p95 12.5 ms |
| Search, last 12 months | p50 8.4 ms, p95 11.6 ms |
| Search, one account + last 12 months | p50 5.2 ms, p95 6.8 ms |
| **Index RAM** (int4 codes + scales + metadata) | **88.8 MB** |
| Process RSS growth after loading (includes SQLite's mapped pages of semantic.db, which the OS can reclaim) | 201 MB |
| Open + load semantic.db into memory | 1.3 s (on the indexer thread; search is keyword-only until then) |
| Insert 600,000 vectors | 5.3 s (113,000 vectors/s) |
| semantic.db on disk | 208 MB (≈ 350 bytes a chunk) |
| Same, with int8 also kept in RAM (`int8_in_ram`) | 245 MB RAM, search p50 18.7 ms: not worth it |

The scan runs on up to 4 threads for indexes over 50,000 chunks. Single-threaded, the all-mail search was 38.8 ms p50 here.

Recall@10 against exact float search over the 600,000 vectors (84 real queries):

| First pass → rescoring | pool 50 | 100 | **200** | 400 | 1,600 |
|---|---:|---:|---:|---:|---:|
| int4 → int8 (what Penguin does) | 0.918 | 0.949 | **0.961** | 0.962 | 0.962 |
| sign bits → int8 | 0.244 | 0.331 | 0.440 | 0.530 | 0.690 |
| sign bits, centered → int8 | 0.301 | 0.382 | 0.495 | 0.595 | 0.754 |
| int8, exhaustive (for reference) | | | 0.962 | | |

- **Tiling is a hard case.** The tiled vectors are near-duplicates of each other, so even exhaustive int8 swaps some near-ties (0.962). The un-tiled real set gives 0.99 (section 4).
- **Pool size.** A pool of 200 (Penguin's minimum) reaches the exhaustive int8 ceiling.
- **Sign bits don't scale.** They lose badly at 256 dimensions, and worse as the corpus grows.

### Model: EmbeddingGemma 300M, 4-bit, ONNX Runtime 1.28 CPU

Before and after this round of tuning (sections 5–7), measured in two interleaved passes on the same afternoon (bench.rs's 172 passages, 115 tokens on average, embedded in one call):

| | 1 thread | 2 threads | 4 threads |
|---|---:|---:|---:|
| Passages/s, before | 1.2 / 1.3 | 2.2 / 2.5 | 3.6 / 4.0 |
| **Passages/s, after** | **1.6 / 1.7** | **2.8 / 2.9** | **4.7 / 4.4** |
| Query p50 / p95, before | 77 / 100, 74 / 95 ms | 57 / 78, 49 / 59 ms | 43 / 58, 42 / 57 ms |
| **Query p50 / p95, after** | **74 / 98, 70 / 90 ms** | **51 / 66, 49 / 59 ms** | **40 / 49, 38 / 49 ms** |

| | before | after |
|---|---:|---:|
| Load model + tokenizer | 2.8–3.2 s | 2.8–2.9 s |
| Resident growth after loading | 414–415 MB | 219–226 MB |
| Resident growth after indexing | 471–520 MB | 291–297 MB |

- About +20% passages/s on this VM (3.8 → 4.55 on average at 4 threads), −8% query p50, −14% query p95, and −45% resident memory while loaded. The throughput gain is the batching (section 5); the memory is the tokenizer trim, its word cache, and smaller runs (section 7).
- **Apple silicon** (GitHub's 3-core virtual M1, 3 threads, section 5): 22–28 passages/s on the harness passages, query p50 16.5–19.7 ms. It indexed the harness mailbox with a third less CPU than the old batching.
- On multilingual-e5-small int8, this VM does about 4–7× the passages per second (section 1's tables), which is the cost of EmbeddingGemma's quality.

**What indexing time means for a full mailbox.** At this VM's 4.5 passages/s, 600,000 chunks would take ~37 hours of indexing time (44 before). Newest mail goes first, so the last months are searchable by meaning within the first hour or so, and search uses the partial index meanwhile.
- **On a Mac:** the 3-core runner embedded 1,260–1,600 tokens/s (22–28 of the harness's 57-token passages a second, 3–4× this VM on the same passages). Real mail's windows are about twice as long (bench.rs averages 115 tokens), so that is roughly 11–14 passages/s, or 12–15 hours of indexing time for 600,000 chunks, spread over the pacing (80% of the time on AC, 20% on battery). A Mac with 4 performance cores for the embedder should do better; real hardware is still to be measured (below).
- **Faster still:** WebGPU (section 6) takes indexing off the CPU; on the runner's paravirtualized GPU it already matched the CPU.
- **Queries (~20 tokens)** are 16.5–19.7 ms p50 on the runner, under the 20 ms target, and 38–40 ms on this VM.

### The whole pipeline on the email eval set

`tests/real_model.rs` runs the eval set through the production path: `chunk_mail` → EmbeddingGemma 4-bit → first 256 dims → int4/int8 index → `search_messages`. Results: paraphrase R@1 0.85 / R@5 0.97 (39 queries), cross-language 0.86 / 0.93 (7), exact identifiers 1.00 / 1.00 (6). These are a little under the 768-dim float numbers in section 1 (R@1 0.92 on paraphrase), which is the cost of 256 dims and quantization together. Query embedding in that test: p50 39–41 ms on this VM, 17–18 ms on the Mac runner.

### Search quality

Section 1 has the model comparisons and the search-eval harness results. On the harness, fusing EmbeddingGemma with keyword search raises nDCG@10 from 0.505 to 0.876 and leaves no query without results.

The embedding changes in sections 5–7, on the same harness (`search-eval run --mode penguin --embedder gemma`, same day, same corpus):

| | before | after |
|---|---:|---:|
| nDCG@10, all 200 queries (Linux) | 0.894 | 0.893 (p = 0.50; 0 better, 2 slightly worse, both `name`) |
| identifier / operator / natural (Linux) | 0.997 / 0.967 / 0.987 | 0.997 / 0.967 / 0.987 |
| nDCG@10 on the Apple-silicon runner | 0.901 | 0.898 (p = 0.35; 7 better, 7 worse) |
| Indexing the mailbox, Linux, 4 threads | 2,160 s, 6,510 CPU-s | 1,663 s, 5,092 CPU-s |
| Indexing the mailbox, Mac runner, 3 threads | 898 s, 1,995 CPU-s | 772 s, 1,355 CPU-s |
| Peak resident memory of the run, Linux | 614 MB | 511 MB |
| Search p50 / p95 (Linux) | 35.2 / 43.0 ms | 34.9 / 44.8 ms |

## Tests

- `cargo test -p penguin-semantic`: chunking (opening, windows, overlap, tail folding, caps, bulk mail, signature/footer/URL/blob cleanup), quantization (int8 cosine error < 0.01, odd lengths, zero vectors, bit packing), the index (filters before the cut, collapse to messages, replace/remove/account removal, the predicate path, persistence and model change), model specs (pinned, consistent) and checksum verification.
- `cargo test -p penguin-core store::semantic`: newest-first paging, spam and drafts skipped, quote stripping, inline images not counted as attachments, bulk flag.
- `cargo test -p penguin-desktop --lib semantic`: the sweep over a real store (1,200 messages), new mail through the top pass, deletions and spam at the next pass, resuming from a saved cursor, account removal, the setting and unload behaviour, the power policy.
- `PENGUIN_SEMANTIC_MODEL_DIR=… cargo test -p penguin-semantic --features onnx --release --test real_model -- --nocapture`: the real model end to end. Recall@5 on the eval set through the quantized index, the account filter, unit-length vectors, and, with a query every 80 ms during indexing, query latency and vectors that match an undisturbed run (cosine > 0.995, each nearest its own passage). Skipped without the model files.
- `PENGUIN_SEMANTIC_MODEL_DIR=… PERF_TEXTS=… cargo run -p penguin-semantic --release --features onnx --example embed_perf`: the speed and memory measurements of sections 5–7. `.github/workflows/macos-embed.yml` runs them on Apple silicon, with WebGPU compiled in.
- `cargo test -p penguin-semantic --lib embed`: the token-budget batch planner.
- `node --test tests/semantic.test.ts`: the Settings progress line.

## Not verified on a Mac yet

The index was built and measured on a shared Linux x86-64 VM. The model's speed, memory and search quality were also measured on GitHub's macOS runner (a 3-core virtual M1, sections 5–7). Still to check on a real Mac:

- **Throughput and latency on real M-series cores**, with 4 embedding threads on a machine with 8–12 cores, and how the QoS classes place them. The runner's 22–28 passages/s and 17 ms queries are from 3 virtual cores.
- **Spinning** (section 5): on the runner it made queries ~40% faster at similar CPU per passage. Measure energy per passage with `powermetrics` on battery before turning it on.
- **WebGPU on a real GPU** (section 6): passages/s, energy, and whether GPU indexing makes the WebKit UI stutter. Build with `--features embed-webgpu`.
- **Memory:** the footprint of the loaded model in Activity Monitor, and that it stays flat across idle unloads and reloads. The runner showed a 144 MB tokenizer footprint, flat across reloads with the word cache off in two runs out of three.
- The added binary size of the static ONNX Runtime in the notarized app, and that notarization is unaffected.
- The power signals in `semantic/power.rs` (IOKit battery state, `thermalState`, `isLowPowerModeEnabled`) on real hardware.
- The first-run download through Hugging Face's CDN from the app's HTTP client.
