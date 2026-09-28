# Ask your inbox

Ask answers questions about your mail on your Mac. No language model writes the answer and nothing is sent anywhere. It works in three steps:

1. A fixed grammar reads the question, in English or Spanish.
2. The answer comes from exact queries over the index and over facts pulled out of mail when it was indexed. Topic questions get quoted sentences instead.
3. Every claim cites the email it came from.

When Ask isn't sure, it shows its sources rather than a guess. When a question needs every matching email (a count, a sum), it reads every one of them. It never works from a sample.

Code: `crates/penguin-core/src/structured/` (extraction), `src/store_extracted.rs` (storage and the scanner), `src/ask/` (understanding and answering), `apps/desktop/src-tauri/src/{ask,extract}.rs` (command and background task), `apps/desktop/src/features/search/{AskCard,AskFacts}.tsx`, `askFormat.ts`, `ask.css` (UI), `src/lib/mock/ask.ts` (mock answers of every kind).

## How a question is answered

```
question ─► normalize (case, accents, contractions, Spanish date words)
         ─► date scope ("this year", "en 2025", date:…)
         ─► template grammar → intent + slots
             │
             ├─ exact ─► people/companies (resolve.rs) ─► SQL over messages,
             │           threads, and the `extracted` table
             │           (flights, stays, orders, parcels, bills, bookings,
             │           contact details, verification codes)
             │           → cards, sums with their parts, counts
             │
             └─ topic ─► FTS5 ∪ vector index ─► reciprocal-rank fusion
                         ─► sentences of the top emails ─► score (words,
                         density, meaning, answer type) ─► quote the best,
                         or show the closest emails and say so
```

### What it can answer

The table below lists the question types, with examples of each.

| Kind | Examples | Answer |
|---|---|---|
| Flights | "when is my flight to Lisbon?", "my next flight", "what's my confirmation code for the flight to Lisbon", "when did I fly to Boston", "¿cuándo es mi vuelo a Lisboa?" | Flight card(s): route, times, confirmation code, the other legs of the same booking, and other upcoming trips to the same place ("1 more upcoming flight to Lisbon") |
| Stays | "where am I staying in Porto?", "when do I check in", "¿dónde me hospedo en Oporto?" | Stay card: check-in and check-out, address, confirmation. A stay counts as "in Lisbon" when its name or address says so, in any of the city's names, or when it starts the day a flight to Lisbon lands (a guesthouse booking that never names the city) |
| Parcels | "where's my Paperleaf order?", "tracking for my order from Hearth & Loom", "has my rug shipped?", "¿dónde está mi paquete?" | Parcel card: carrier, tracking number (check-digit verified), progress, expected date, a "Track on UPS" link |
| Orders | "what did I order from Paperleaf", "my recent orders" | Order cards: number, items, total, status |
| Bills | "what bills are due?", "when is my Brightwave bill due?", "how much do I owe Ledgerly", "¿cuándo vence la factura de la luz?" | Bill cards, soonest first. A bill is paid when a later email from the same biller says so for the same invoice number or amount |
| Bookings | "when is my dinner reservation", "my tickets for The Lanterns" | Reservation card: when, party size, venue, confirmation |
| Spending | "how much did I spend at Uber this year?", "how much did I spend on flights in 2025", "¿cuánto gasté en Uber este año?" | "$412.18 across 9 Uber receipts in 2026", the math (per currency), what was left out and why, and the full list of receipts |
| Counts | "how many orders did I place with Paperleaf this year", "how many flights did I take in 2025", "how many emails from Priya" | The count, with duplicates merged, plus the total when the items have amounts |
| Codes | "latest verification code from Rydeo" | The code and its age. The app shows the code; the CLI and MCP never do |
| Contact details | "what's Priya's phone number?", "Dana's address", "¿cuál es el teléfono de Lucía?" | The value as written in their signature, how many emails it appears in, and a person card. "Who is Priya" shows it too |
| What someone said | "what did Priya say about pricing?", "what did we decide about the vendor contract", "¿qué dijo Priya sobre los precios?" | Their sentences, quoted, with date and a link |
| Did they reply | "did Priya reply about the contract?" | Yes (with the first line of the reply) / "Not yet: you wrote last on …" |
| Find | "find the lease from Dana", "find the offsite agenda" | The matching emails, with attachments whose names match listed first |
| Anything else | "what is the wifi password for the offsite?", "how do I reset my Ledgerly password" | The best sentence(s), quoted; else "No sentence in your mail clearly answers that" and the closest emails |

The earlier question types still work unchanged: last and first contact, relationship span, latest item, open loops, who is, top senders, and when.

## 1. Structured extraction at index time

`structured::extract(&MailInput) -> Vec<Found>` is pure and deterministic. It runs over one message's subject, sender, authored text (quoted history removed) and raw HTML part.

### Sources, most trusted first

1. **schema.org JSON-LD** (`<script type="application/ld+json">`).
2. **schema.org microdata** (`itemscope` / `itemtype` / `itemprop`). It is read into the same JSON shape, so one mapper handles both. Gmail accepts both formats ("Gmail supports both JSON-LD and Microdata", [Get started](https://developers.google.com/workspace/gmail/markup/getting-started)). Property values follow the WHATWG rules ([HTML §5.2.4](https://html.spec.whatwg.org/multipage/microdata.html)):
   - `meta` → `content`
   - `a`/`area`/`link` → `href`
   - `img`/`audio`/`video`/`source`/`track`/`iframe`/`embed` → `src`
   - `object` → `data`
   - `data`/`meter` → `value`
   - `time` → `datetime`
   - anything else → its text

   Senders also put `content` on other elements, so that is accepted as well.
3. **Text patterns** for mail without markup. This is most mail: Google shows markup only from registered senders, who need "a high volume of mail … (order of hundreds of emails a day minimum to Gmail) for at least a few weeks" ([Register with Google](https://developers.google.com/workspace/gmail/markup/registering-with-google)). So markup in the wild comes almost entirely from large senders.

Markup wins. Patterns only add kinds of fact the markup didn't cover.

### schema.org types and properties read

Gmail's reference ([overview](https://developers.google.com/workspace/gmail/markup/overview)) and schema.org sometimes spell the same thing differently. Every reader accepts both spellings, and accepts values as strings, numbers, nested objects, or arrays of these.

| Type | Properties | Spelling notes |
|---|---|---|
| [FlightReservation](https://developers.google.com/workspace/gmail/markup/reference/flight-reservation) | `reservationNumber`, `reservationStatus`, `underName`; `reservationFor` → [Flight](https://schema.org/Flight): `flightNumber`, `airline{name, iataCode}`, `departureAirport` / `arrivalAirport{name, iataCode}`, `departureTime` / `arrivalTime`; `totalPrice` + `priceCurrency` | Gmail's `reservationNumber` = schema.org's `reservationId` ([Reservation](https://schema.org/Reservation)) |
| [LodgingReservation](https://developers.google.com/workspace/gmail/markup/reference/hotel-reservation) | `reservationFor` (LodgingBusiness `name`, `telephone`, `address`), check-in/out, `reservationNumber`, `totalPrice` | Gmail: `checkinDate` / `checkoutDate`; [schema.org](https://schema.org/LodgingReservation): `checkinTime` / `checkoutTime` |
| FoodEstablishmentReservation ([Gmail](https://developers.google.com/workspace/gmail/markup/reference/restaurant-reservation)), [EventReservation](https://developers.google.com/workspace/gmail/markup/reference/event-reservation), RentalCarReservation, TrainReservation, BusReservation | `startTime` / `startDate`, `partySize`, `reservationFor{name, location{name, address}}`, `reservationNumber` | — |
| [Order](https://developers.google.com/workspace/gmail/markup/reference/order) | `merchant` or `seller`, `orderNumber`, `price` + `priceCurrency` (else the sum of `acceptedOffer` prices in one currency), `acceptedOffer.itemOffered.name`, `orderStatus` | [schema.org Order](https://schema.org/Order): `merchant` is superseded by `seller`; Gmail still requires `merchant` |
| [ParcelDelivery](https://developers.google.com/workspace/gmail/markup/reference/parcel-delivery) | `trackingNumber`, `trackingUrl` (https only), `carrier`, `expectedArrivalUntil`, `partOfOrder{orderNumber, merchant}`, `deliveryStatus` | `trackingNumber` is only "recommended" in Gmail's reference |
| [Invoice](https://developers.google.com/workspace/gmail/markup/reference/invoice) | `provider`, `totalPaymentDue` (a [PriceSpecification](https://schema.org/PriceSpecification)), `paymentStatus`, `confirmationNumber` / `accountId` | Gmail: `paymentDue`; [schema.org Invoice](https://schema.org/Invoice): `paymentDueDate` |

Times keep the local wall time as written ("2026-10-02T19:05"), because departure and check-in times are local to the place. Statuses map schema.org enumerations (`ReservationCancelled`, `OrderInTransit`, `PaymentComplete`, …) to a small set.

### Text patterns

Each extractor needs a cue before it looks at all: a subject word such as "order", "invoice" or "itinerary", or a labeled field. Then it reads labeled values in English and Spanish:

- Order numbers: "Order #", "Número de pedido".
- Confirmation codes: "Confirmation code", "Record locator", "Localizador".
- Amounts due: "Amount due", "New balance", "Importe a pagar".
- Due dates: "Due date", "Payment due by", "Fecha de vencimiento".
- Stays: "Check-in" / "Check-out".
- Tables: "table for 4", "mesa para 2".

**Flights.** Flight numbers must be a known IATA airline designator with a "flight" cue or a matching sender, or any designator right after the word "flight". IATA calls these "2-letter code" airline designators and "3-letter code" locations ([IATA code search](https://www.iata.org/en/publications/directories/code-search/)). Airports come from an explicit pair ("SFO → LIS", "JFK - LAX", "(MAD) → (JFK)") or from two known codes. A 140-airport, 70-airline table (`structured/places.rs`) maps city names, including Spanish ones, to codes, so "flight to Lisboa" finds LIS.

**Stays** need a check-in (or "arrive") date and either a check-out date or a number of nights ("7 nights"). **Orders** need a total: a shipping notice that only repeats the order number adds nothing to add up. **Tickets** need a reference or a price, so a friend's "tickets for Thursday?" isn't a booking. **Bills** read an amount due, balance, premium or "Amount:" line, or "invoice INV-1 for $X"; a due date written out, or "net 30" / "due in 30 days" counted from the email's date. A later "payment received" for the same invoice number or amount marks one paid.

**Tracking numbers** must pass the carrier's check digit:

| Carrier | Format | Check | Source |
|---|---|---|---|
| UPU S10 (international post) | 2 letters, 8 digits, check digit, country | Weights 8 6 4 2 3 5 9 7, sum mod 11 subtracted from 11 (10 → 0, 11 → 5) | [UPU S10 standard](https://www.upu.int/UPU/media/upu/files/postalSolutions/programmesAndServices/standards/S10-12.pdf) |
| USPS IMpb | 20/22 digits, 91–95 prefix | Mod 10, weights 3 and 1 from the right | [USPS Publication 199](https://postalpro.usps.com/pub199) (MOD 10 steps in [v10.1](https://postalpro.usps.com/storages/2016-12/782_PUB199IMPBImpGuide.pdf)) |
| UPS | "1Z" + 16 | Mod 10 over the 15 characters after 1Z, letters (A=2 … Z=7), even positions ×2 | No primary UPS source is public; the de-facto spec is [jkeen/tracking_number_data](https://github.com/jkeen/tracking_number_data) ([CHECKSUM_ALGORITHMS.md](https://raw.githubusercontent.com/jkeen/tracking_number_data/main/CHECKSUM_ALGORITHMS.md)) |
| FedEx Express | 12 digits, FedEx named | Weights 3 1 7 over 11 digits, sum mod 11 mod 10 | same repo, [fedex.json](https://raw.githubusercontent.com/jkeen/tracking_number_data/main/couriers/fedex.json) |
| DHL Express | 10 digits, DHL named | First nine as a number, mod 7 | same repo |
| Amazon Logistics | "TBA" + 12 digits, Amazon named | none | — |

A number printed right after "Tracking number" is accepted for any carrier, marked unverified.

**Contact details** come only from a person's own message (not bulk, not an automated sender, no other fact found), from the last 14 lines of what they wrote. That is where signatures sit.

- **Phones** need a leading "+", parentheses or two separators, and 8–15 digits (E.164's maximum). Dates and runs of a longer number are rejected. This is deliberately narrower than [libphonenumber](https://github.com/google/libphonenumber)'s `findNumbers`, because a signature is the only place Penguin trusts a number to be the person's.
- **Addresses**: a US street line (number, name, suffix) with an optional "City, ST 12345" next line, or a Spanish "Calle/Avenida … n, 28013 Madrid".

**Amounts** are read by `ask/extract.rs`:

- Currency markers go before or after the number ($, US$, €, £, ₱, ₹, ¥, and ISO codes: [ISO 4217](https://www.six-group.com/en/products-services/financial-information/market-reference-data/data-standards.html), maintained by SIX).
- The decimal separator is whichever comes last with 1–2 digits after it, so "1.234,56 €" and "$1,234.56" both read right. CLDR notes that separators vary by locale ([CLDR number patterns](https://cldr.unicode.org/translation/number-currency-formats/number-and-currency-patterns)).
- The receipt total is the most specific "total" line: "grand/order total" > "amount charged/paid" > "total" > "amount due". Subtotals and savings never count.

**Dates in text** are read in English and Spanish: "Sep 24", "24 September 2026", "15 de octubre de 2026", ISO dates, 9/24/2026, and day-first when the first number can't be a month (24/09/2026). Weekdays ("Thursday", "el viernes") resolve against the email's date. Times attach too ("at 7pm", "a las 21:00").

### Storage and the scanner

Schema migration `SCHEMA_EXTRACTED`, the last one in `MIGRATIONS`:

- `messages.extracted` is NULL until the message is read, then holds the extractor version.
- A partial index `messages_unextracted(date) WHERE extracted IS NULL` makes "what's left" one probe.
- Table `extracted(msg, ord, kind, account_id, date, at, key, ref, amount, currency, source, data)`, indexed by `(kind, date)`, `(kind, at)`, `(key, kind)` and `ref`. `data` is the fact as JSON; the other columns make it queryable.
- A trigger drops a message's facts when the message is deleted: account removal, re-store, purge.
- `extract_state.version`: raising `EXTRACTOR_VERSION` re-reads everything once.

`Store::extract_pending(max)` reads unextracted messages newest first in batches of 100. Bodies are read on a pooled reader and extraction runs outside the write lock. Each short write transaction marks the batch and stores its facts; a message deleted or re-stored meanwhile is skipped. The app's task (`src-tauri/src/extract.rs`):

- starts 15 s after launch;
- runs passes of 400 messages, 150 ms apart, while there is a backlog;
- then sleeps until local mail changes (sync, or a body downloaded when a thread opens; `AppState::mail_changes`), waits 2 s for the burst to settle, and runs a pass. That covers new mail and downloaded bodies (a re-stored message is NULL again) without polling;
- runs at utility QoS on macOS (`src-tauri/src/qos.rs`).

It logs only counts and timings.

While the first backfill runs, questions that name something also extract, in memory, the unscanned messages a search for it finds. The answer carries `coverage` ("Still reading 1,204 emails for bookings …") and its confidence is lowered.

**Cost:** 0.29 ms per message at 300k (88 s for the whole backfill, in the background), 0.02 ms for a pass with nothing to read, +2.5 MB of database. See [Numbers](#numbers).

## 2. Query understanding

`ask/intent.rs` is a template grammar:

- literal words;
- `(a|b c|)` alternatives;
- `<name:…>` recorded alternatives;
- `{slot}` captures;
- `%MACRO%` word lists.

The first matching template wins, and a validator rejects captures made only of glue words. There are 25 intents (24 question kinds plus "unknown") and 243 templates:

- `TEMPLATES_FIRST`: facts, person + topic, Spanish.
- The original `TEMPLATES`.
- `TEMPLATES_LAST`: find, open questions.

**Normalization** does the following:

- lowercases;
- folds accents ("cuándo" = "cuando"; Spanish templates are written without them);
- expands contractions;
- splits possessives ("priya's" → "priya 's");
- strips ¿ ¡;
- drops filler words ("please", "por favor").

It then turns Spanish date phrases into the English ones the date parser reads: "el año pasado" → "last year", "este mes" → "this month", "en 2025" → "in 2025", "desde marzo" → "since march". The date scope is the trailing phrase or a `date:` / `after:` / `before:` operator. It is shown as a removable interpretation ("Dates: “this year” = Jan 1 – Dec 31, 2026").

**Slots** are resolved as follows:

- People and companies go through `resolve.rs`: names, addresses, domains (Linden ↔ linden.example), acronyms (KOTK), account nicknames and profile names. The previous answer's person stands in for "he" / "they".
- Places go through the airport table.
- Merchants are matched by the sender domain's organization label ("noreply@email.uber.example" → "uber") and by extracted merchant names.
- Spending categories ("flights", "hotels", "vuelos") read the bookings.

## 3. Answering

### Exact answers

These are SQL over the index and the `extracted` table. Every card, sum part and count element is a cited email.

- **Sums** (`how much did I spend at X`):
  1. Resolve the merchant to its domain and read every email from it in the date range.
  2. For each email, take the extracted amount (an order or receipt total, a booking total, or a bill marked paid), else its receipt total line.
  3. Count one order once: a confirmation and a shipping notice with the same order number add once, and the second is reported as a duplicate. Refunds are negative.
  4. Leave out invoices still due, and say so. If there are only invoices, sum them as "billed", not "spent".
  5. Never convert currencies; show one total per currency.

  The answer reads "$78.25 across 4 Uber receipts this year". The card shows the total, what each amount is, what was left out, and the full list with each email's amount and the line it came from.
- **Counts** of orders, flights, stays and parcels merge emails about the same thing, by reference number or flight number and date.
- **Travel** chooses upcoming or past by the question ("when do I fly" = next; "when did I fly" = last; otherwise the next, else the latest) and groups a booking's legs by confirmation code.

### Topic answers (passages.rs)

This is the extractive design from open-domain QA: a retriever narrows the collection, then a reader selects the answer span from the retrieved text ([Chen et al., ACL 2017, DrQA](https://aclanthology.org/P17-1171/)). Penguin's "reader" is a sentence scorer, not a neural model.

1. **Retrieve.** For open questions, first the input exactly as typed, ranked by search itself, so a search typed into Ask ("account:studio invoice", "1,284") answers like search. Then FTS5 search with all of the question's content words, widened to any of them when that finds fewer than 8 conversations. When the app provides the embedding model and vector index (`AskSemantic`, penguin-semantic's `Embedder` / `VectorIndex`), a second list comes from the vector index, filtered by account set, date range and (for "what did X say") X's messages. The two lists are fused by reciprocal rank, score = Σ 1/(60 + rank) ([Cormack, Clarke & Büttcher, SIGIR 2009](https://doi.org/10.1145/1571941.1572114): "k = 60 was fixed during a pilot investigation"). Dense and sparse retrieval complement each other: [DPR](https://arxiv.org/abs/2004.04906) reports BM25 59.1 vs dense 78.4 top-20 accuracy on Natural Questions, while its hybrid isn't always better. RRF needs no score calibration between the two.
2. **Read** the authored text of the top 12 emails (quoted history is left out; the subject counts as a sentence). Split it into sentences at . ! ? or a line break, not after abbreviations. This is a simplified [UAX #29](https://www.unicode.org/reports/tr29/) that, like UAX #29 itself, needs an abbreviation list for "Mr. Jones". Boilerplate (unsubscribe, "sent from my …") is dropped.
3. **Score** each sentence:
   - **words:** the share of the question's term weight it contains, with BM25's idf over the candidate sentences ([Robertson & Zaragoza 2009](https://doi.org/10.1561/1500000019)) and prefix stems ("renewal" ~ "renew").
   - **density:** how close together the matched terms sit. [Tellex et al. (SIGIR 2003)](https://dl.acm.org/doi/10.1145/860435.860445) found "the best algorithms … employ density-based measures for scoring query terms".
   - **meaning:** cosine similarity to the question, for the 96 best sentences by words, when there is a model. They are embedded ahead of background indexing, like a search query (docs/SEMANTIC.md, "Ask's sentences"), and the question's own vector comes from the search query cache.
   - **answer type:** whether it holds what the question asks for. "When" needs a date, "how much" an amount, "how many" a number, "where" a place, "who" a person. These are the coarse answer types of [Li & Roth (COLING 2002)](https://aclanthology.org/C02-1150/) (NUMERIC:date, NUMERIC:money, LOCATION, HUMAN …), and a sentence without the needed type is penalized.
   - **retrieval rank** of its email.

   Weights without a model: 0.7 words + 0.18 density; with a model: 0.45 words + 0.12 density + 0.35 meaning. Both add ±0.12/0.25 for answer type and 0.1/(1+rank).
4. **Answer.** Quote up to three sentences from different emails, each with sender, date, subject and a link, the question's words highlighted. A sentence must carry at least half the question's term weight (or be close in meaning, ≥ 0.75, with some shared words) and have the right answer type. Otherwise the answer is "No sentence in your mail clearly answers that" with the closest emails. For questions that sound like totals ("all", "total", "list"), it points to the exact counting questions.

**Confidence** reads as follows:

- **Exact:** computed over every matching email, from markup or verified patterns.
- **Likely:** pattern-extracted, a quoted sentence covering ≥ 75% of the question with the right answer type, or a count or sum while the scanner is still reading.
- **Best guess:** a partial match.
- **No answer:** nothing matched.

**Fact questions fall back to the text.** When a flight, stay, parcel, order, bill, booking or phone-number question finds no extracted fact, Ask answers it as a topic question. "How much is the new rent?" finds the landlord's sentence when no rent bill was extracted. If the text has nothing either, the fact answer ("I couldn't find …") stands.

**Never invented.** Ask does not paraphrase, combine sentences or compute a number from retrieved samples. RAG answers "what does the evidence say", not "how many things match" (`research/13-summaries-and-ask-your-inbox.md` §B.5), so counting and summing questions always go to the exact path.

## 3b. Questions as queries

Templates read one phrasing each. "How many flights did I take in 2025" had a template, but "how many times did I fly in august" didn't, so it became a topic question. The query layer reads a question by its parts instead, and answers it exactly.

**The schema** (`ask/query.rs`, `AskQuery`):

| Part | Values |
|---|---|
| subject | flights, stays, orders, parcels, bills, bookings, spending, messages |
| op | count, sum, average, max, min, list, first, last, next, exists |
| measure | items, money, nights, trips |
| groupBy | month, year, merchant, place, person |
| filters | timeframe, place, merchant, person, direction, field, tense |
| comparison | two timeframes or two names |

Timeframes are kept as written and read by the date grammar (`read_timeframe`), plus quarters, "next month", "the next 3 months", "year to date" and "from last year". A timeframe the grammar can't read fails the check, so no one can invent a range.

**The grammar** (`ask/qparse.rs`) is compositional. It takes a question apart, then puts the pieces together:

1. Normalize as for the templates, including Spanish date phrases.
2. Take out comparisons ("in July or August", "Lisbon vs Madrid") and date phrases, from anywhere in the question.
3. Look up each word in the lexicon.
   - Verbs and nouns give the subject: fly, flew, flown, trip, vuelo, volé → flights; stay, hotel, nights, noches → stays; order, bought, receipts, rides → orders; package, deliveries → parcels; bill, invoice, factura → bills; reservation, tickets → bookings; spend, paid, cost, gasté → money; emails, wrote → messages.
   - "Go", "went", "visited" become a trip when a known place follows.
   - Cue words give the operation: how many / cuántos → count; how much / total → sum; average; biggest / cheapest / longest; first / last / next; when; "did I ever" → exists; "which month … most" → group by month plus max; "per month"; "where do I fly the most".
   - Verbs give the tense: "did", "have taken" and "flew" are past; "do I have", "upcoming" and "for October" look ahead.
4. Read what follows a preposition as an entity. It's a place when the airport table (or a country) knows it. Otherwise it's a merchant, or a person for messages.

The grammar takes a question only when it names a subject and every content word was understood. Anything else stays with the templates and passages. The templates remain the fast path for one thing (the next flight, where a parcel is). Counts, sums, dated lists, groups and comparisons go to the query layer, which checks every matching fact.

**The executor** (`ask/query_exec.rs`) reads every fact of the subject and merges them into one unit per real thing, using the same keys as the counts. Then it filters by time, tense, place, merchant and person, and computes the answer:

- A count or list is "Flights in August 2026: 3", with a card for every flight.
- A sum is per currency.
- An average is per item, or per month when grouped by month (empty months count).
- Max and min pick the item.
- Groups are sorted rows, with the winner marked.
- A comparison says which side has more, and by how much.
- Exists is a yes or no, with the latest item.

Along the way:

- Money is dated by its email.
- A flight to X lands in X, and flights home aren't destinations.
- Parcels are matched to a shop through its order numbers.
- For a merchant that only sends bills, "how much did I pay" is what it billed.

The answer carries `understood` (the reading), `groups` and `result`, a machine-readable value that says the same as the headline.

**Reading with the on-device model.** Some phrasings the grammar can't read, such as "did I take a Swiftcab yesterday" or "how many round trips did I book". For those, the app asks Apple's on-device model (`ask_understand`, Settings → AI → "Read questions with Apple Intelligence", on by default when available). The model fills the same schema through guided generation (`@Generable struct MailQuery` in the Swift bridge); it doesn't answer. Apple's guidance shaped the design:

- **Constrained output.** "Constrained sampling prevents the model from producing malformed output" ([Generating Swift data structures with guided generation](https://developer.apple.com/documentation/foundationmodels/generating-swift-data-structures-with-guided-generation)). Every enum field is `@Guide(.anyOf([…]))`, and the free text (timeframe, place, merchant, person) is checked by Rust.
- **Code does the counting.** The on-device model is "not designed for world knowledge or advanced reasoning" ([WWDC25 286](https://developer.apple.com/videos/play/wwdc2025/286/)), and Apple advises against using it for math ("Non-AI code is much more reliable for math", [WWDC25 248](https://developer.apple.com/videos/play/wwdc2025/248/)). So the model only reads the question, and SQL computes the answer.
- **Instructions are ours; the question goes in the prompt.** Instructions are fixed and developer-written, one to three paragraphs with a few short examples. The question goes only in the prompt, fenced. Apple: "don't include input from people or any unverified input in the instructions" ([Improving the safety of generative model output](https://developer.apple.com/documentation/foundationmodels/improving-the-safety-of-generative-model-output)). It also says "Aim for one to three paragraphs" ([Instructions](https://developer.apple.com/documentation/foundationmodels/instructions)) and "Try giving the model between 2-15 examples" ([Prompting an on-device foundation model](https://developer.apple.com/documentation/foundationmodels/prompting-an-on-device-foundation-model)).
- **Small and fast.** The schema is small and the response is capped at 160 tokens, well inside the 4,096-token window ([TN3193](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)). Temperature is 0, and there's a 10 s timeout. It shares the summaries' one-at-a-time gate, and the token budget is checked with `tokenCount(for:)` on macOS 26.4+.
- **Checked before it runs.** `QueryDraft::into_query` validates every enum, reads the timeframe with the date grammar, and rejects impossible combinations (nights of flights). The executor then refuses a merchant, place or person that nothing in the mail matches. A reading that fails either check is dropped, and the grammar's answer stays.
- **Availability.** Checked with `SystemLanguageModel.default.availability`. Without Apple Intelligence nothing changes; the grammar path is the whole feature.

**The card** shows the reading as chips: "Understood as: Flights · to Lisbon · Aug 1 – Aug 31, 2026 · How many".

- The subject, operation and grouping are menus.
- The dates can be typed ("since march").
- A place, store, person or grouping has an ×.
- A change runs `ask_query` with the edited reading and replaces the answer.

The source line says whether Penguin's grammar, Apple Intelligence or you wrote the reading. Grouped counts and sums, and the two sides of a comparison, render as rows with bars.

## 4. The Ask card

`AskCard.tsx` shows the headline, detail, a coverage note, then the rich part for the answer's kind, from `AskFacts.tsx`:

- **Flight card:** IATA codes and cities with the route between them, local times, departure day, confirmation code with a copy button, passenger, total; "2 emails" when a booking was re-sent.
- **Stay, order, bill and booking cards:** the key fields, and a status chip ("In 6 days", "Due in 3 days", "Overdue", "Paid", "Cancelled").
- **Parcel card:** a four-step progress bar (shipped → in transit → out for delivery → delivered), the tracking number with a copy button, the expected date, and "Track on UPS", which opens the carrier's page in the browser on click.
- **Sum block:** the total per currency, what it's made of, and what was left out. The list of every email added is folded to three rows until you ask for more, or until the keyboard selection walks into it.
- **Quoted passages:** a left rule, the question's words highlighted, and the sender · date · subject underneath.
- **Person card** for "who is" and "what's X's phone": avatar, addresses, phone and postal address from their signature.

Each card says where it came from ("schema.org" or "text"). Keyboard selection moves over the answer's cited emails (the search overlay's existing list keys), and the card or passage for the selected email is highlighted. Enter opens it, ⌘-Enter keeps the overlay. `src/lib/mock/ask.ts` has an answer of every kind for `npm run dev:mock` ("when is my flight to Lisbon", "where's my package", "what bills are due", "what did Priya say about pricing", "what is the wifi password", "what's Priya's phone number", "latest verification code").

## 5. The semantic seam

Ask depends only on penguin-semantic's traits. `Store::ask_with(question, scope, now, offset, Some(AskSemantic { embedder, index }))` turns on retrieval and sentence ranking by meaning; `Store::ask` is the same without. The app's `ask` command calls `Store::ask` until the real model and index are in `AppState`. Wiring them is one line in `src-tauri/src/ask.rs`. The stand-ins (HashEmbedder / FlatIndex) test the plumbing (`ask::tests_facts::meaning_search_through_the_semantic_traits`).

## 6. Privacy

- Everything runs locally and read-only (it also works on `Store::open_read_only`, as the MCP server opens the database).
- Nothing about a message is logged: the scanner logs counts and timings only.
- Verification codes appear in answers only with `AskScope::reveal_codes`. The app sets it; the CLI and MCP don't, so codes stay out of their output as before.
- "Track on …" opens the carrier's public page only when clicked.

## How other products do it

- **Shortwave** ([deep dive](https://www.shortwave.com/blog/deep-dive-into-worlds-smartest-email-ai/)) runs these steps, with 3–5 s end to end:
  1. An LLM reformulates the question and extracts features (dates, names, labels) with confidences.
  2. Full-text/metadata search and vector search (Instructor embeddings) run in parallel.
  3. Heuristic reranking: Gaussian date filters, boosts for contacts and labels, promotions pushed down, recency.
  4. A cross-encoder (MS MARCO MiniLM) cuts ~1,000 candidates to a few dozen.
  5. An LLM writes the answer.

  Penguin keeps the retrieval half, with a grammar and resolver where Shortwave uses an LLM, and quotes instead of generating.
- **Gmail** summary cards ([help](https://support.google.com/mail/answer/15195630?hl=en)) cover orders, events, travel, bills and promotions, "generated based on the information from your email", with a "Based on [x] email" link. Gmail Q&A ([announcement](https://workspaceupdates.googleblog.com/2024/08/gmail-q-new-way-of-searching-your-inbox.html)) answers questions like "What was the PO number for my agency?" with Gemini. The retrieval design isn't published. Penguin's cards follow the same idea: the fact, and the email it came from.
- **Apple** `NSDataDetector` ([docs](https://developer.apple.com/documentation/foundation/nsdatadetector)) finds "dates, addresses, links, phone numbers, and transit information" (flight numbers: `airline`, `flight`) in natural text, and warns it "discards potential matches in case of uncertainty". Penguin's patterns take the same stance. Apple Intelligence's Mail features (priority messages, summaries) aren't documented at the level of retrieval.
- **Superhuman** Ask AI indexes up to five years of mail through a third-party vendor (help center, not retrievable here). Penguin keeps the index on the Mac.
- **Duckling** ([facebook/duckling](https://github.com/facebook/duckling)) is the reference for rule-based dates and amounts of money with a grain ("day", "hour"). Penguin's date and amount rules are smaller and email-specific, and keep local wall times the same way.

## Numbers

### Extractors

Three measurements, from most to least favorable. Only the first set was written together with the extractors. Fact-level: an extracted fact counts as correct only when its kind and every key field match (for a flight: number, confirmation, airports, departure time; for a bill: amount and due date). Wrong fields and facts nobody asked for count against precision; facts not found count against recall.

| Set | What it is | Precision | Recall | Key fields |
|---|---|---:|---:|---:|
| Tuning fixtures, `structured::tests::precision_and_recall` | 24 fictional emails: 7 with schema.org (JSON-LD, microdata), 11 plain text in English and Spanish, 6 negatives (promotions, an airline sale, a personal note, a colleague mentioning an invoice, a code, meeting notes with numbers). Written together with the extractors. | 1.000 | 1.000 | 1.000 |
| Held-out fixtures, `held_out_precision_and_recall` | 16 emails with new senders, layouts and phrasings, written after the extractors were tuned: a round trip in a JSON-LD array, a table-layout e-ticket, a Spanish flight, a vacation rental, a two-column order, a refund, FedEx, DHL, a numeric due date, a "payment received", event tickets, a Spanish restaurant, a Spanish signature, 3 negatives. **First run: 0.667 / 0.571 (key fields 0.776).** The misses were order numbers and tracking numbers only in the subject, carrier names only in the sender, a paid invoice without a due date, tickets read as an order, and "a las 21:00". Fixed generally, then: | 1.000 | 1.000 | 1.000 |
| The eval corpus, `penguin-eval/tests/extraction.rs` | 19,183 messages written by another agent without these extractors in mind. Of its 6,431 conversations, 1,110 carry ground truth tags (order number and amount, tracking number, booking code, confirmation number, invoice number). 275 carry JSON-LD. Everything else (work threads, newsletters, promotions, codes, invites, family mail) should yield no booking/order/bill facts. **First run: 0.956 / 0.902.** Hotels gave "N nights" instead of a check-out; received invoices said "Amount:" and "net 30"; airports appeared as "(SFO) → Lisbon (LIS)"; shipping updates made orders without a total; a friend's "tickets for Thursday" became an event. Fixed generally, then (tuned against this set, so optimistic): | 0.999 | 0.998 | — |

The eval corpus after the fixes, per kind:

| Ground truth | Conversations | Correct | Missed | Other facts | From markup |
|---|---:|---:|---:|---:|---:|
| receipt → order | 624 | 623 | 1 ("your bike is ready, pay at pickup") | 0 | 245 |
| bill → bill (or receipt) | 174 | 173 | 1 (a domain renewal without a price) | 0 | 0 |
| shipping → shipment | 123 | 123 | 0 | 0 | 0 |
| flight → flight | 96 | 96 | 0 | 0 | 30 |
| hotel → stay | 62 | 62 | 0 | 0 | 0 |
| invoice received → bill | 31 | 31 | 0 | 0 | 0 |
| everything else | 5,321 | — | — | 1 (a friend's forwarded dinner reservation) | — |

### Ask on the search eval (docs/SEARCH-EVAL.md)

`search-eval run --mode ask` scores the cited emails of each answer, in order, over the 200 judged queries. "Before" is `crates/penguin-eval/baselines/ask.json` (same corpus fingerprint `589bcd9d20e27164`). "After" is this branch, with the corpus through the fact scanner as in the app. `--no-extract` gives the same scores: facts extracted on the fly during the first backfill give the same answers.

| Category | n | nDCG@10 before → after | MRR Δ | R@10 Δ | Wins / losses | p50 ms Δ |
|---|---:|---|---:|---:|---|---:|
| identifier | 23 | 1.000 → 1.000 | +0.000 | +0.000 | 0 / 0 | +1.6 |
| misspelling | 17 | 0.038 → 0.514 * | +0.507 | +0.512 | 12 / 0 | +2.6 |
| name | 17 | 0.702 → 0.880 * | +0.000 | +0.229 | 14 / 0 | +2.5 |
| natural | 24 | 0.865 → 0.969 | +0.062 | +0.083 | 5 / 0 | +4.3 |
| operator | 26 | 0.735 → 0.894 * | +0.038 | +0.181 | 17 / 0 | +1.7 |
| paraphrase | 33 | 0.000 → 0.388 * | +0.346 | +0.412 | 18 / 0 | +3.6 |
| question | 28 | 0.143 → 0.533 * | +0.339 | +0.393 | 14 / 0 | +5.1 |
| spanish | 18 | 0.254 → 0.520 * | +0.167 | +0.258 | 12 / 0 | +2.7 |
| time | 14 | 0.307 → 0.512 * | +0.161 | +0.221 | 6 / 0 | +1.7 |
| **all** | 200 | **0.442 → 0.687 \*** | **+0.186** (0.525 → 0.711) | +0.258 | 98 / 0 | +3.1 |

`*` p < 0.05, paired randomization test. p95 latency over all 200 queries: 13.0 ms.

The eval is a retrieval measure: did the answer cite the right emails, in the right order? It doesn't score whether the headline states the right date or amount; the end-to-end tests below do that.

- **Where the gains come from.** Questions gain from the fact answers (flights, stays, bills, parcels) and quoted sentences. Misspellings and paraphrases gain from widening to any of the question's words. Operators, names and identifiers gain from searching the input as typed first.
- **Still failing (nDCG 0).** "how much did I pay for the standing desk?" (the receipt names the product model, not "standing desk"), "when is ben's wedding?", "when am I on call?", "when is my winter vacation?", "what time is the plumber coming?", "who is the adjuster on my insurance claim?". These are vocabulary gaps that need the embedding model, or questions no template reads that fall back to passages whose words don't match.

### Understanding and end to end

- 167 question → intent-and-slots cases (`ask::intent::tests`), 79 original plus 88 new, including 21 in Spanish. All pass.
- `ask::tests_facts` has 11 end-to-end tests on a fictional mailbox. Fact questions are asked twice, before and after the scanner, and must give the same headline. They cover:
  - flight (plus return leg, confirmation code, past flight, Spanish, unknown city);
  - stay;
  - parcel and orders;
  - spending (one order counted once, the math, Spanish);
  - bills (due vs paid);
  - bookings and "when is …";
  - codes (hidden without `revealCodes`);
  - phone numbers and "who is";
  - quoted passages (wifi password, what Priya said, a question nothing answers);
  - did-they-reply;
  - counts;
  - meaning search through the penguin-semantic traits.
- All 237 penguin-core tests pass. `tests/ask.test.ts` covers the card formatting.

### Cost

- Pure extraction: 33,000–47,000 messages/s on one core (release, eval corpus mix).
- Through the store, including reads and writes: the eval corpus (19,183 messages) takes 0.8–0.9 s, and 1,135 facts are stored.
- At 300,000 messages (`bench_ask`): the backfill reads 300,000 messages in 88 s, 0.29 ms each. The bench corpus is mostly large HTML newsletters, and both the text and HTML parts are decompressed. It adds 2.5 MB to the database.
- A pass with nothing to read: 0.02 ms. It is one probe of the partial index, named with `INDEXED BY`: the planner's statistics can date from before the scan, and the table scan it picked instead cost 81 ms.
- The app spreads the backfill over 400-message passes 150 ms apart (about 120 ms of work each): roughly 3–4 minutes of background time for 300k messages. It never holds the writer for more than a short transaction of 100 messages.

Ask latency at 300,000 messages (`PENGUIN_BENCH_N=300000 cargo run -p penguin-core --release --example bench_ask`, read-only store, 20 runs; a shared Linux box, so tails are noisy). New question types:

| Question | p50 ms | p95 ms |
|---|---:|---:|
| when is my flight to Lisbon | 48 | 65 |
| where is my package | 27 | 37 |
| what bills are due | 14 | 16 |
| what did I order from Uber | 5 | 5 |
| how much did I spend on Uber this year (269 receipts, the math) | 60 | 73 |
| ¿Cuánto gasté en Uber este año? | 60 | 76 |
| what did the team say about the budget (passages) | 35 | 46 |
| what is the wifi password for the offsite (passages) | 48 | 61 |
| why is the sky blue (passages, nothing answers) | 39 | 61 |

The existing questions are unchanged within noise. The slowest is still `how many emails about the budget` at 81 / 100 ms, which this work doesn't touch. Before these changes, the Uber spend question took 58 / 86 ms.

### Numbers: questions as queries

The question set is `ask-eval`, 380 questions from real sources ([ASK-QUESTIONS.md](ASK-QUESTIONS.md)). "Before" is the base commit 03385ee with the same eval binary. The corpus is generated for Sun 2026-09-27; a question counts as correct when its exact expected value is right.

| Category | Tuning n | Before | After, grammar | After, + parser (simulated) | Held-out n | Before | After, grammar | After, + parser |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| count | 78 | 22% | 99% | 100% | 15 | 20% | 67% | 80% |
| aggregate | 70 | 23% | 100% | 100% | 14 | 14% | 93% | 100% |
| temporal | 47 | 43% | 100% | 100% | 9 | 33% | 89% | 100% |
| lookup | 30 | 57% | 100% | 100% | 5 | 20% | 40% | 60% |
| list | 24 | 0% | 96% | 96% | 5 | 0% | 100% | 100% |
| existence | 23 | 17% | 96% | 100% | 5 | 0% | 80% | 100% |
| comparison | 20 | 0% | 100% | 100% | 5 | 0% | 100% | 100% |
| people | 12 | 42% | 100% | 100% | 2 | 50% | 100% | 100% |
| summary | 14 | 86% | 86% | 86% | 2 | 50% | 50% | 50% |
| **all** | **318** | **28.6%** | **98.4%** | **99.1%** | **62** | **17.7%** | **80.6%** | **90.3%** |

- **The tuning set was used to build the grammar, so read the held-out column as the estimate for new phrasings.** The first run of the query layer on the tuning set scored 71.7%, which shows how much tuning went in.
- **Coverage of each path.** The grammar answers 98.1% of tuning questions and 90.3% of held-out questions with an exact answer (not quoted sentences). The rest are the model path's to take.
- **The parser column is not the model.** Apple Intelligence doesn't run in CI's macOS VMs or on Linux, so it simulates a perfect reading: each missed question's gold query goes through `Store::ask_draft`, the path the model's output takes. It measures the plumbing and bounds what the model can add. The model's real accuracy on these phrasings is unmeasured.
- **Held-out misses** tell where the grammar stops:
  - "number of flights I took in july" and "number of Gearloft orders in 2025": a contact-info template ("number … of X") catches them first.
  - "how many round trips…", "how many parcels arrived…", "what's the most I've paid for a single order", "when did I last check in to a hotel", "did Tom email me last month": no reading.
  - "confirmation code for my Lisbon flight" and "order number for my most recent Gearloft purchase": the templates misread the modifier.
  - "flights to London this year: how many?" counted today's flight.
  - "what did Mike say about the parking spot": two Mikes.
- **Latency** over all 380 questions, median of 3 runs each: before p50 3.9 ms, p95 11.0 ms; after p50 4.1 ms, p95 27.1 ms, max 93 ms. The query layer reads every fact of a subject; the slowest questions are money questions over every order and booking.
- **The existing Ask eval** (`search-eval run --mode ask`, 200 queries, fresh before and after) moved from nDCG@10 0.683 to 0.698. There were 5 wins ("flights last summer", "swiftcab rides last month", "who is the adjuster on my insurance claim?", …) and 1 loss ("deskcraft order confirmation" 1.00 → 0.83; the right email is still first).

## Limits

- **Question coverage.** The grammar reads the question shapes in `ask-eval` and their variants. New phrasings land about 80% of the time (held-out). Without Apple Intelligence, the rest become topic questions. With it, the model's reading is checked first and dropped when it doesn't check out.
- **Money questions** are about what the mail says was paid. Autopay bills with no receipt count as billed, not spent, and there's no currency conversion.

- **Grammar coverage.** A question phrased in a way no template reads becomes a topic question: it still gets quoted sentences or sources, not an exact answer. Spanish covers the common forms (last contact, spend, counts, parcels, flights, stays, bills, who/what said, phone numbers), and answers are in English.
- **Patterns** handle common layouts in English and Spanish. Other languages, image-only receipts, and totals inside images or PDFs aren't read; attachments aren't parsed. Flight emails without markup need a flight number, and hotel emails need both check-in and check-out.
- **Airport and airline tables** are compact (≈140 airports, 70 airlines). Unknown airports are still read from explicit pairs, but "flight to Tbilisi" can only match an airport name written in the email.
- **Tracking**: UPS and FedEx check digits come from the community-maintained spec, since there is no public primary source. Carriers beyond UPS/USPS/FedEx/DHL/Amazon/S10 are read only when labeled.
- **Money**: no currency conversion. Totals are per currency.
- **Paid bills** are inferred from a later "payment received" email for the same invoice number or amount. Autopay without an email looks unpaid.
- **Meaning** needs the real embedding model; with the stand-in or none, passage answers rely on shared words (plus stems), so "plane tickets" won't find "flight" by meaning.
- **Did they reply** reads only the best-matching thread.
- **Times** are the local wall times written in the email. "In 6 days" compares dates on your calendar, not time zones.

## Sources

Primary sources were checked on 2026-09-26. Pages that couldn't be loaded are marked.

**Email markup and schema.org**
- Google, Email Markup: [Get started](https://developers.google.com/workspace/gmail/markup/getting-started) (JSON-LD and microdata), [Overview](https://developers.google.com/workspace/gmail/markup/overview), [Register with Google](https://developers.google.com/workspace/gmail/markup/registering-with-google).
- Google, Email Markup reference: [Flight](https://developers.google.com/workspace/gmail/markup/reference/flight-reservation), [Hotel](https://developers.google.com/workspace/gmail/markup/reference/hotel-reservation), [Restaurant](https://developers.google.com/workspace/gmail/markup/reference/restaurant-reservation), [Event](https://developers.google.com/workspace/gmail/markup/reference/event-reservation), [Order](https://developers.google.com/workspace/gmail/markup/reference/order), [Parcel delivery](https://developers.google.com/workspace/gmail/markup/reference/parcel-delivery), [Invoice](https://developers.google.com/workspace/gmail/markup/reference/invoice).
- schema.org types: [Reservation](https://schema.org/Reservation), [LodgingReservation](https://schema.org/LodgingReservation), [EventReservation](https://schema.org/EventReservation), [Flight](https://schema.org/Flight), [Order](https://schema.org/Order), [ParcelDelivery](https://schema.org/ParcelDelivery), [Invoice](https://schema.org/Invoice), [PriceSpecification](https://schema.org/PriceSpecification).
- WHATWG HTML, [§5.2 Microdata](https://html.spec.whatwg.org/multipage/microdata.html) (property values: §5.2.4).

**Identifiers**
- UPU, [S10: Identification of postal items, 13-character identifier](https://www.upu.int/UPU/media/upu/files/postalSolutions/programmesAndServices/standards/S10-12.pdf).
- USPS, [Publication 199](https://postalpro.usps.com/pub199) (Intelligent Mail package barcode; MOD 10 steps in [v10.1](https://postalpro.usps.com/storages/2016-12/782_PUB199IMPBImpGuide.pdf)).
- [jkeen/tracking_number_data](https://github.com/jkeen/tracking_number_data) (MIT): UPS and FedEx check digits, which have no public primary spec.
- [IATA code search](https://www.iata.org/en/publications/directories/code-search/) (2-letter airline and 3-letter location codes; "controlled duplicate" designators).
- ISO 4217 via its maintenance agency, [SIX](https://www.six-group.com/en/products-services/financial-information/market-reference-data/data-standards.html) (iso.org returned 403).
- ITU-T [E.164](https://www.itu.int/rec/T-REC-E.164/en) (not loadable here; the 15-digit maximum is as commonly documented, e.g. by libphonenumber).

**Parsing**
- [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) (date-times).
- [Unicode UAX #29](https://www.unicode.org/reports/tr29/) (sentence boundaries).
- [CLDR number and currency patterns](https://cldr.unicode.org/translation/number-currency-formats/number-and-currency-patterns).
- [google/libphonenumber](https://github.com/google/libphonenumber).
- [facebook/duckling](https://github.com/facebook/duckling).
- Apple, [NSDataDetector](https://developer.apple.com/documentation/foundation/nsdatadetector) and [transitInformation](https://developer.apple.com/documentation/foundation/nstextcheckingresult/checkingtype/transitinformation).

**Retrieval and question answering**
- Tellex, Katz, Lin, Fernandes, Marton, [Quantitative evaluation of passage retrieval algorithms for question answering](https://dl.acm.org/doi/10.1145/860435.860445), SIGIR 2003 ([PDF](https://groups.csail.mit.edu/infolab/publications/Tellex-etal-SIGIR03.pdf)).
- Chen, Fisch, Weston, Bordes, [Reading Wikipedia to Answer Open-Domain Questions](https://aclanthology.org/P17-1171/), ACL 2017.
- Karpukhin et al., [Dense Passage Retrieval for Open-Domain Question Answering](https://arxiv.org/abs/2004.04906), EMNLP 2020.
- Cormack, Clarke, Büttcher, [Reciprocal Rank Fusion outperforms Condorcet and individual Rank Learning Methods](https://doi.org/10.1145/1571941.1572114), SIGIR 2009 ([PDF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf)).
- Robertson & Zaragoza, [The Probabilistic Relevance Framework: BM25 and Beyond](https://doi.org/10.1561/1500000019), FnTIR 2009.
- Li & Roth, [Learning Question Classifiers](https://aclanthology.org/C02-1150/), COLING 2002.
- Yang, Yih, Meek, [WikiQA](https://aclanthology.org/D15-1237/), EMNLP 2015 (answer sentence selection).

**Products**
- Shortwave, [A deep dive into the world's smartest email AI](https://www.shortwave.com/blog/deep-dive-into-worlds-smartest-email-ai/).
- Gmail, [Use summary cards](https://support.google.com/mail/answer/15195630?hl=en) and [Gmail Q&A](https://workspaceupdates.googleblog.com/2024/08/gmail-q-new-way-of-searching-your-inbox.html).
- Superhuman, [Ask AI](https://help.superhuman.com/hc/en-us/articles/38458628979091-Ask-AI) (returned 403; described from search snippets).
