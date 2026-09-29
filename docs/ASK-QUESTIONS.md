# What people ask their mail: sources, taxonomy, and the Ask question set

Ask's question set is built from questions people actually ask, not ones we wrote. This page lists where they come from, how they're grouped, how each became a question about the synthetic mailbox, and how Ask does on them.

- The set lives in `crates/penguin-eval/src/bin/ask-eval/`. `questions.tsv` holds 353 questions and `heldout.tsv` holds 77. Each row names its source, its category, the question, and a gold query.
- `oracle.rs` computes each expected answer from the corpus itself: its tags and the text it generated. It never uses Penguin's extractors or Ask.
- `main.rs` runs and grades the set, and `draft.rs` holds the simulated model parser.
- Run: `scripts/box-cargo.sh run -p penguin-eval --release --bin ask-eval -- --parser`. `--probe FILE` asks each line of FILE and prints the answer, the reading and the result, ungraded: the way to see what a new phrasing does before writing its gold query.

How Ask answers is in [ASK.md](ASK.md), under "Questions as queries".

## Sources

Collected 2026-09-27. Every question below is quoted from the page linked. A key in parentheses is the `source` column of the question files.

### Products

| Key | Source | Questions quoted |
|---|---|---|
| `pogue-35`, `pogue-65`, `pogue-18`, `pogue-11`, `pogue-conf`, `pogue-flightno`, `pogue-avg`, `pogue-dinner`, `pogue-until`, `pogue-reply` | David Pogue, [125 tests of the new AI Siri](https://pogueman.substack.com/p/125-tests-of-the-new-ai-siri), 2026-09-17. These are real queries a reviewer typed, with his pass/fail marks. | "How many flights have I taken this year?" (#35, answered "You have taken 11 flights so far this year" plus a list) · "How much have rides cost me this year?" (#65) · "When's the last time I went to Illinois and where did I stay?" (#18) · "Did Jeffrey's box ever arrive?" (#11) · "Confirmation number for my flight today?" · "What's my flight number to Austin?" · "What's the total of these invoices? What's the average?" · "When's my dinner with the Mutis?" · "How much time until my flight?" · "Did I reply to that email about the Italian printing of my book?" |
| `gws-spend`, `gws-po` | Google Workspace Updates, [Gemini in the side panel of Gmail](https://workspaceupdates.googleblog.com/2024/06/gemini-in-side-panel-of-gmail.html), 2024-06-24, and [Gmail Q&A](https://workspaceupdates.googleblog.com/2024/08/gmail-q-new-way-of-searching-your-inbox.html), 2024-08-29 | "How much did the company spend on the last marketing event?" · "What was the PO number for my agency?" |
| `gmail-package`, `gmail-nextflight`, `gmail-from` | Gmail Help, [Collaborate with Gemini in Gmail](https://support.google.com/mail/answer/14355636?hl=en&co=GENIE.Platform%3DDesktop) | "When is my package arriving?" · "What time is my next flight?" · "Emails from [person] sent last week." |
| `gaio-flight`, `gaio-conf`, `gaio-tracking`, `gaio-tickets`, `gaio-checkin`, `gaio-trip` | Gmail Help, [Get an AI Overview in Gmail search](https://support.google.com/mail/answer/16789526?hl=en) | "When is my flight to Hawaii?" · "Confirmation number for my flight tomorrow." · "Tracking number for the laptop shipment." · "Show me my concert tickets." · "What's the check-in code for my rental?" · "Summarize my upcoming trip." |
| `gws26-paid` | Google Workspace Updates, [AI Overviews in Gmail search](https://workspaceupdates.googleblog.com/2026/04/search-faster-and-smarter-with-ai-overviews-in-Gmail-search.html), 2026-04 | "Which invoices have I already paid and which are still outstanding to Sandbox Supplies?" |
| `gblog-plumber` | Google, [Gmail is entering the Gemini era](https://blog.google/products-and-platforms/products/gmail/gmail-is-entering-the-gemini-era/), 2026-01-08 | "Who was the plumber that gave me a quote for the bathroom renovation last year?" |
| `tg-people`, `tg-date` | Tom's Guide, [I'm using Gemini now for my Gmail](https://www.tomsguide.com/ai/i-am-using-gemini-now-for-my-gmail-and-theres-one-major-discovery-thats-surprising), 2025-06-01, and [I reset my Gmail inbox…](https://www.tomsguide.com/ai/i-reset-my-gmail-inbox-for-the-new-year-with-these-5-handy-ai-prompts), 2026-01-01 | "the people I emailed the most this year and also in May" (reported wrong in the test) · "Which topics did I discuss and reply to the most in 2025" (failed: it listed newsletters) · "What date did Alex Hughes say he was going to send the AI report?" |
| `ms-said`, `ms-next` | Microsoft Learn, [Draft engaging emails using Copilot in Microsoft Outlook](https://learn.microsoft.com/en-us/training/modules/from-inbox-impact-improve-your-email-workflows-ai/2-draft-engaging-emails), and Microsoft Support, [Chat with Copilot in Outlook](https://support.microsoft.com/en-us/outlook/copilot-outlook/chat-with-copilot-in-outlook) | "What did [name] say about the budget?" · "When is my next 1:1 meeting with /name?" |
| `sw-receipts`, `sw-emailed`, `sw-land`, `sw-offsite`, `sw-landlord`, `sw-invoices` | Shortwave: [AI Assistant docs](https://www.shortwave.com/docs/guides/ai-assistant/), [The new Shortwave AI Assistant](https://www.shortwave.com/blog/new-shortwave-ai-email-assistant/) (2024-09-09), [A deep dive into the world's smartest email AI](https://www.shortwave.com/blog/deep-dive-into-worlds-smartest-email-ai/) (2023-10-17), and a [third-party review](https://cmdk.email/post/shortwave-review/) (2026-07-22) | "List the receipts I got last week, including the amount of each purchase" · "Which of them have I emailed this year?" · "When does Jonny land in Phoenix?" · "Where was our last offsite held?" · "what did the landlord say about the deposit" · "summarize every invoice from this vendor this year" |
| `sh-flight`, `sh-staying`, `sh-offsite`, `sh-help-vs` | Superhuman, [Introducing Ask AI](https://blog.superhuman.com/ask-ai/) (2024-05-22) and the [Ask AI help article](https://help.superhuman.com/hc/en-us/articles/46005676610829-Ask-AI) | "when is my flight" · "where am I staying" · "where is the q2 offsite" · "How much time did I spend in meetings vs. deep work this month?" |
| `spark-flight`, `spark-prepare` | Spark, [Meet your personal AI Assistant](https://sparkmailapp.com/blog/spark-ai-assissant) (2025-11-07) | "When is my flight to Rome?" · "What do I need to prepare for my trip?" |
| `canary` | Canary Mail, [Copilot](https://canarymail.io/features/ai) | "Show me invoices from last month." |
| `plai-invoices`, `plai-accountant` | [protonmail-local-ai](https://github.com/marshalltech81/protonmail-local-ai) README | "Find all invoices from last year and extract the vendor names and amounts" · "What did my accountant say about Q3 taxes?" |
| `apple-mom` | Apple Newsroom, [Introducing Apple Intelligence](https://www.apple.com/newsroom/2024/06/introducing-apple-intelligence-for-iphone-ipad-and-mac/), 2024-06-10 | "When is Mom's flight landing?" |

### The maintainer

| Key | Source | Questions quoted |
|---|---|---|
| `owner-totals`, `owner-first`, `owner-contact`, `owner-extra` | Justin Gluska, 2026-09-29, asking for more in Ask | "I wanna be able to say things like 'total cost of my linear receipts this year' and it adds up … Or 'when did I hire X' or 'their email' and it finds the first email communication etc." Rewritten for the corpus (Linear → Streamly, Tunewave and Nimbus Cloud, which send monthly receipts and bills) with the variants people type: "Streamly total this year", "sum of my Nimbus invoices", "how much do I pay for Streamly a month", "how long have I known Priya", "oldest email with Priya", "Priya's email", "how do I reach Dana", "Mike's email". `owner-extra` are the question types added alongside (subscriptions, the largest receipt, emails from someone this year, the last one from them). |

### People asking on Hacker News

Reddit couldn't be fetched (the search tool refuses the domain, direct fetches were blocked), so forum questions come from Hacker News only.

| Key | Thread | Question |
|---|---|---|
| `hn-conf` | [42088048](https://news.ycombinator.com/item?id=42088048), 2024-11-08 | "What's the confirmation number for my flight next week?" |
| `hn-leave` | [42431551](https://news.ycombinator.com/item?id=42431551), 2024-12-16 | "When does my flight leave?" |
| `hn-mexico`, `hn-claim` | [49789939](https://news.ycombinator.com/item?id=49789939), 2026-09-21 | "when did I arrive and leave Mexico City on my trip at the end of 2024?" · "What was the quote for my insurance claim?" |
| `hn-autopay` | [45342030](https://news.ycombinator.com/item?id=45342030), 2025-09-23 | "Find all emails with 'autopay' in the subject from my utility company for the past 12 months, then compare it to the prior year's data." |
| `hn-electric` | [49276585](https://news.ycombinator.com/item?id=49276585), 2026-08-12 | "what was my last electricity bill?" (Grok "failed to find it, even when I told it the exact subject line") |
| `hn-electric2` | [36055195](https://news.ycombinator.com/item?id=36055195), 2023-05-24 | "what was my electricity bill last month" |
| `hn-ccbill` | [39397317](https://news.ycombinator.com/item?id=39397317), 2024-02-16 | "when is my credit card bill due and how much is it" |
| `hn-sf` | [34598406](https://news.ycombinator.com/item?id=34598406), 2023-01-31 | "When's my next flight to SF and what's the confirmation #?" |
| `hn-tracking` | [45422150](https://news.ycombinator.com/item?id=45422150), 2025-09-30 | "what's the tracking number for my package?" |
| `hn-garbanzo` | [41632643](https://news.ycombinator.com/item?id=41632643), 2024-09-24 | "When did I last buy 10kg of garbanzo beans, and from where? What price did I pay?" |
| `hn-receipts` | [46900462](https://news.ycombinator.com/item?id=46900462), 2026-02-05 | "find all the receipts in my emails for this year…" |
| `hn-booking` | [46992177](https://news.ycombinator.com/item?id=46992177), 2026-02-12 | "show me tonight's booking" |

### Datasets and studies

| Key | Source | What it contributes |
|---|---|---|
| `lme-*` | LongMemEval (Wu et al. 2024, [arXiv 2410.10813](https://arxiv.org/abs/2410.10813)). Personal chat history, with multi-session and temporal-reasoning questions. | The count, sum and "which … most" shapes, verbatim: "How many plants did I acquire in the last month?" (`lme-plants`) · "How many times did I bake something in the past two weeks?" (`lme-bake`) · "How much total money have I spent on bike-related expenses since the start of the year?" (`lme-bike`) · "What is the total amount I spent on luxury items in the past few months?" (`lme-luxury`) · "Which airline did I fly with the most in March and April?" (`lme-airline`) · "Which grocery store did I spend the most money at in the past month?" (`lme-grocery`) · "How many different types of food delivery services have I used recently?" (`lme-delivery`) · "How many months ago did I book the Airbnb in San Francisco?" (`lme-airbnb`) · "When did I book the Airbnb in Sacramento?" (`lme-sacramento`, an abstention case) · "How many different museums or galleries did I visit in December?" (`lme-museums`, answer 0) · "How many months have passed since I last visited a museum with a friend?" (`lme-museum-since`) · "What is the order of airlines I flew with from earliest to latest before today?" (`lme-order`) · "What is the order of the three trips I took in the past three months…" (`lme-trips`) |
| `locomo-cities` | LoCoMo (Maharana et al. 2024, [arXiv 2402.17753](https://arxiv.org/abs/2402.17753)) | "Which cities has Jon visited?" |
| `enronqa-*` | EnronQA (Ryan et al. 2025, [arXiv 2505.00263](https://arxiv.org/abs/2505.00263); [dataset](https://huggingface.co/datasets/MichaelR207/enron_qa_0922)) | Single-email lookups: "What is the reservation code and the passenger's last name required to access the trip plans…, according to the email from Hawaiian Airlines?" · "What is the name of the resort where the sender plans to stay…?" · "What is the phone number that can be called to place an order with Flowers USA…?" |
| (context) | Ai, Dumais, Craswell & Liebling, *Characterizing Email Search using Large-scale Behavioral Logs and Surveys* (WWW 2017; only the abstract could be fetched). Carmel et al., *Rank by Time or by Relevance?* (CIKM 2015). Mackenzie et al., *Exploring User Behavior in Email Re-Finding Tasks* (WWW 2019). Elsweiler & Ruthven (SIGIR 2007). | How people search mail: names in about 40% of queries, 1 to 1.5 terms, operators in under 1%, and dates when keywords fail. Elsweiler & Ruthven's diary study found re-finding tasks were 60% lookup, 35% known-item and 3% multi-item. These studies shape the lookup and people categories and the time phrases. They don't supply count questions. |

What the sources say, in short:

- **Count and aggregate questions are what people want, and no email benchmark has them.** EnronQA's question generator bans them outright: "The question should NOT include counting, math, or any other operations. For example … 'how many recipients were there?' are not allowed." They show up in reviewers' tests (Pogue's "How many flights have I taken this year?"), in vendor demos (Google's "How much did the company spend…"), in user wishes on Hacker News, and in memory benchmarks (LongMemEval, LoCoMo, MemBench). When Tom's Guide tried "the people I emailed the most" on Gemini, it got them wrong.
- **Nearly every question carries a time phrase:** this year, last week, next, latest, since, "in the past few months".
- **Travel is the most common example:** flights, confirmation codes, hotels, "where am I staying".
- **Lookups keep coming back:** confirmation, PO, tracking and order numbers, the last bill's amount.

## Taxonomy

| Category | What it asks | Examples from the sources | In the set |
|---|---|---|---|
| **count** | How many things, emails or nights | "How many flights have I taken this year?", "How many plants did I acquire in the last month?" | 79 + 15 |
| **aggregate** | Totals, averages, extremes, "which … most", per-month breakdowns | "How much have rides cost me this year?", "Which grocery store did I spend the most money at…?", "What's the average?", "total cost of my linear receipts this year" | 83 + 20 |
| **temporal** | When, first, last, next, how long since or until | "When's the last time I went to Illinois…", "What time is my next flight?", "How much time until my flight?", "when did I hire X" | 59 + 14 |
| **lookup** | One value: a code, a number, an amount, an address | "Confirmation number for my flight today?", "Tracking number for the laptop shipment.", "What was the PO number…?", "their email" | 38 + 8 |
| **list** | Every item in a set | "List the receipts I got last week…", "Find all invoices from last year…", "Which cities has Jon visited?", "what subscriptions do I pay for" | 25 + 6 |
| **comparison** | More or less between two periods or names | "…compare it to the prior year's data", "meetings vs. deep work this month" | 20 + 5 |
| **existence** | Yes or no | "Did Jeffrey's box ever arrive?", "…museums … in December?" (answer 0) | 23 + 5 |
| **people** | Who, the most | "the people I emailed the most this year", "Who was the plumber…?" | 12 + 2 |
| **summary** | Questions that need retrieval rather than facts | "what did the landlord say about the deposit", "Where was our last offsite held?" | 14 + 2 |

The counts read tuning + held-out.

## From source questions to mailbox questions

Each source question was rewritten in the corpus's terms. The shape stays: the verb, the time phrase, the "most", the comparison. The entities become ones the mailbox has:

- **Cities:** the 12 in `corpus/world.rs` (Lisbon, Madrid, Valencia, Chicago, Denver, Seattle, Austin, Boston, Toronto, Mexico City, London, Tokyo) and their countries.
- **Merchants:** Swiftcab rides, Dishdash, Beanhouse Coffee, Parcelmart, Gearloft, Pagebound Books, Green Grocer, App Market, Mercado Sol, Deskcraft.
- **Billers:** Cedar Valley Power, Brightwave Internet, Harbor Mobile, Pinecrest Water, Crestline invoices, Streamly.
- **Airlines and hotels:** Skyward Air, Pacifica Airlines, Aerovía; Staywell, Nestaway.
- **People:** the cast.

Some examples:

- Pogue's "How much have rides cost me this year?" became "How much have my Swiftcab rides cost me this year?".
- "Illinois" became Chicago, Seattle or London.
- "Which grocery store did I spend the most money at in the past month?" became "which store did I spend the most at last month".
- "Did Jeffrey's box ever arrive?" became "did I get a package from Deskcraft".

Several variants of one source question differ only in their time phrase or entity. The `source` column says which question each one came from.

Spanish variants (¿Cuántas veces volé en agosto?, ¿Cuánto gasté en agosto?, ¿En qué mes gasté más este año?, …) are translations of the same source questions. The corpus has Spanish mail and a Spanish-speaking family.

**Held-out set.** `heldout.tsv` (62 questions) was written after the grammar and executor were tuned on `questions.tsv`, and nothing was changed to make it pass. It rephrases the same sources ("how often did I fly in august", "number of Gearloft orders in 2025", "which merchant got the most of my money last month"). Its score is the honest estimate for new phrasings.

## Definitions the expected answers use

The corpus is generated for a fixed day, **Sun 2026-09-27, noon local**. So "in August" is August 2026, "last year" is 2025 and "last summer" is June to August 2026. The expected answers follow these rules, which are also Penguin's:

- **Flights.** A round trip is two flights: out on the departure date and back on the return date. "Flights to X" are the ones landing in X. A trip is one booking. Past-tense questions ("have I taken this year") count flights before today, and "last" means before today.
- **Orders.** Receipts and order confirmations, subscription receipts ("Amount charged") included. A refund isn't an order. "Pay at pickup" isn't paid.
- **Spending.** Receipts, subscription charges and hotel booking totals, with refunds counted negative, dated by the email. Bills that are only due are billed, not spent. At a merchant that only sends bills (a utility), "how much did I pay" is what it billed. Currencies are never added together.
- **Bills.** Statements and received invoices that have an amount due. A renewal notice with no price isn't one.
- **People.** The humans in the cast, not automated senders. "Who did I email the most" counts recipients of your mail.
- **First email.** "When did I hire X", "when did I start working with X" and "how long have I known X" are the day of the first email between you in either direction (sent to them, or copied, counts); "when did X first email me" and "the first email I sent X" pick a direction.
- **Contact details.** A person's email is the address the cast gives them; a phone is one written in their own mail as "(415) 555-0138". A name two people share ("Mike") expects both addresses.
- **Subscriptions.** The merchants whose receipts say "Amount charged" every month (Streamly, Tunewave). What they cost a month is the sum of each one's latest charge. Nimbus Cloud's monthly bills are bills, not subscriptions.
- **"A month."** "How much do I pay for X a month" is the average per month over the twelve whole months before this one (`@last12full`).
- **Where you fly.** Destinations: a flight home to the airport most flights leave from isn't a destination.

## Grading

- **Numbers, amounts and dates.** A count must equal the expected integer, and an amount must match each currency within a cent. A date must be the expected day. Each is read from the answer's `result` (new) or its headline, sum and cards, so the old code is graded the same way. It also has to be an exact answer, not quoted sentences that happen to contain a number.
- **Lookups.** The code or number must appear in the answer.
- **Lists.** The cited conversations must be exactly the expected set.
- **Groups and "which … most".** The winner must match; ties accept any tied label. Per-month breakdowns must match every row.
- **Comparisons.** The winning side must match.
- **Yes/no.** The yes or no must match.
- **Summary.** The conversation that holds the answer must be among the first three cited.
- **Contact details.** Every expected address (or the phone) must appear in the answer: its headline, detail or `result.text`.
- **Subscriptions.** The answer's rows must be exactly the expected merchants; their monthly cost is graded as an amount.
- **Source recall.** Reported separately: the share of the conversations behind the expected answer that the answer cites, capped at 60.

## Results

See [ASK.md](ASK.md#numbers-questions-as-queries) for the tables: before and after, per category, grammar alone and with the parser, and latency; and [the 2026-09-29 numbers](ASK.md#numbers-totals-by-merchant-first-email-contact-details-subscriptions-2026-09-29) for the totals, first-email and contact-details questions (the 15 new held-out questions were written before the code changed).
