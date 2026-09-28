# Search experience

How the search overlay behaves and why. The operator language is in `docs/SEARCH.md`. This page covers what someone sees and does, with the primary sources behind each decision. The sources were fetched and quoted on 2026-09-26. The last section lists what couldn't be fetched.

The goal: search should work "naturally without telling the user how". There are no syntax lessons and no "semantic" labels. People type what they remember and get the mail back.

## Principles and their sources

| Principle | Source (primary) |
|---|---|
| Search on every keystroke, with no debounce for the local index. | Apple HIG, *Search fields*: "If possible, start search immediately when a person types." Superhuman, *Delightful search* (2017): "Search results should appear right away and update as the user types." Raycast developer docs: "Use built-in filtering for best performance" (throttle only async work). |
| A keystroke is answered in under 100 ms, aiming for under 50 ms. | Miller 1968 (AFIPS): feedback within "0.1 to 0.2 seconds"; "Response to control activation … no more than 0.1 second." Nielsen, *Response Time Limits*: "0.1 second is about the limit for having the user feel that the system is reacting instantaneously." Superhuman, *Built for speed* (2022): "aims for latency less than 50ms whenever possible", and Conrad Irwin (2019) groups latencies into "<50ms (fast), <100ms (ok)". web.dev RAIL: "process user input events within 50 ms"; INP counts ≤200 ms as "good" for the web. That is a floor, not our target. |
| Never flash "No results" while the answer is still settling. | Raycast store guidelines: rendering an empty list first causes "a flickering 'No results' view". NN/g, *User Intent Affects Filter Design*: "continuous updates can be visually distracting, even if they are fast." |
| Don't move rows the person is about to act on. | web.dev CLS and the W3C Layout Instability spec: a shift within 500 ms of a keypress is expected (`hadRecentInput`). A later shift is unexpected. Mackenzie et al., WWW '19: some failures were "unforced", meaning the target was on screen and not seen, and 6% of clicks landed on a lower duplicate. |
| Put a small relevance band above a newest-first list. | Mackenzie et al., WWW '19: time-based ranking "begins to fail as email age increases … hybrid approaches may help". Apple Mail: "Top Results is listed first in the results". Apple HIG: "Provide the most relevant search results first … consider categorizing them." Spotlight: "Results appear instantly, with the best match at the top." |
| Meaning is what people remember, so search by meaning is core. It is never labeled. | Elsweiler, Baillie & Ruthven, TOIS 2008: topic was remembered in 85.1% of tasks, the reason for the email in 80.9%, the sender in 77.1%, dates in 57.5% and attachments in 12.8%. Apple Mail: "search results based on what you mean, not just the words you type", with no "semantic" label. Google (2020) ranks passages and shows "the single sentence that answers your question". |
| Plain English turns into visible, removable chips. | Apple HIG: "A token is a visual representation of a search term that someone can select and edit." Linear filters: natural language "to have your views automatically filtered". Slack AI shows "which filters were applied". Gmail chips (2020) refine "without needing to … use search operators". |
| Suggest people, recents and refinements as you type. | Apple HIG: "predictive search suggestions while they're typing … help people search faster and type less." SwiftUI docs: "Remember previous searches and offer the most recent." Gmail (2022) weights contacts by "how often you interact". |
| Keep suggestions small, text-first, and never a dead end. | NN/g, *Site Search Suggestions*: suggestions "that return zero results … are worse than unhelpful". NN/g, *Enriched Site-Search Suggestions*: rich panels "are largely ignored … Do not eliminate simple text autosuggestions." |
| Don't require syntax. | Nielsen, *Search: Visible and Simple*: "Most users cannot use advanced search or Boolean query syntax." Elsweiler et al., SIGIR 2011: advanced features "were rarely used". Mackenzie et al.: operators appeared in under 1% of queries. |
| A no-results page must help. | NN/g, *"No Results" pages*: state it clearly, keep the query, offer "similar queries that do return results" and "spelling corrections". NN/g, *Internal search*: on zero results, "automatically retrieve the results for the alternative-spelling suggestion". Apple Mail: "Search all mailboxes". NN/g, *Scoped search*: "remove any selected scopes with one click." |
| Be honest about indexing. | Apple, *About Spotlight indexing* (2026): indexing "can take hours or even days" and shows "an indexing progress indicator". Card, Robertson & Mackinlay (CACM 1993): give status "at intervals no longer than" 1 s. |

Two corrections to common lore:
- The "Doherty threshold of 400 ms" does not appear in the transcribed 1982 IBM brief. That brief argues for sub-second response, with gains continuing down to 0.3 s. We cite Miller, Nielsen and RAIL for the 100 ms rule instead.
- Apple's HIG never says "don't require syntax". That rule comes from NN/g and the Gmail chips announcement.

## What someone sees

### Typing: instant and steady (`useInstantSearch.ts`, `stable.ts`)

- **Every keystroke searches at once.** The old one-frame wait before sending is gone.
- **One search is in flight at a time.**
  - Keys typed while a search runs collapse into the newest query. It is sent the moment the running search answers, so a slower search over words and meaning never queues work for text that has since been replaced. This is how stale requests are cancelled: they are never sent.
  - The answer that was in flight is still shown, as the closest thing available.
- **Results stay on screen until the next ones arrive.**
  - An empty answer waits 220 ms, with the old rows dimmed, before "no mail" replaces them. Typing through a word such as "leas…" therefore never flashes an empty state (Raycast).
- **Stable merge.**
  - When a new answer arrives for the query already on screen, the selected row and everything above it stay put. Such answers come from matches by meaning landing later, the index growing while it builds, or Back to a search.
  - Rows already shown keep their group and order. New rows slide in below the selection, at the place their rank gives them, with a 200 ms fade and a 4 px glide. Reduced motion turns that off.
  - A new row whose group sits above the selection waits until the selection moves up or the query changes.
  - A different query starts a fresh list with the first row selected. Change within 500 ms of a keypress reads as expected (CLS).
- **The selection follows the row, not its index.** If the selected row goes away, the selection stays in the same place.
- **A still pointer doesn't select.** Rows sliding under a mouse that hasn't moved don't steal the selection. Only real pointer movement selects.
- **While search by meaning is still indexing,** the same query is quietly run again every 3 s. New matches slide in and the status line updates.

### Layout, top to bottom

1. **The answer slot.** A question ("when did I last email Priya?") mounts the Ask card above everything else, like Slack AI answers and Gmail's AI Overviews. The mail behind the answer comes below it.
2. **Top results.** The first result gets the large card. The next two get two-line rows showing the sender, date, subject and the passage that matched. If nothing matches all the words, the band is called "Closest matches" with the hint "No mail has all of these words". That is an honest statement, not a technology label.
3. **Calendar** events, then **Attachments.**
   - Attachments get one row of three cards, or six when the query is about files. Attachments are recalled in only 12.8% of re-finding tasks (Elsweiler et al.), so they don't push threads down.
4. **Threads.** Everything else that matched the words, newest first. This is the time-ordered list people expect under the relevance band (Mackenzie et al.).
5. **Related.** Threads that are only close in meaning, closest first.
6. **From Gmail.** Shown only when asked for (⌘⇧↵).
7. **People** in the right column. Choosing one shows their mail.

### Snippets that explain themselves

- When the words matched, the snippet is the index's own excerpt with the words marked.
- When the thread matched by meaning, the snippet is the passage that matched (`SearchHit.passage`, at most 240 characters). Any query words in it are marked at word starts: "rent" marks "rent" and "rents", never "current". Words under three letters and a short list of function words are never marked.
- Nothing says "semantic". "Rent going up" shows *"there is no change to the rent under the current lease for the coming term"*, and that passage is the explanation.

### Understanding plain English without teaching syntax

- **Suggestions as you type** (`suggest.ts`). They appear on one quiet line under the box, and Tab takes the first:
  - **People**, with avatars. "mi" offers Mike Delgado and Mike Osei. "invoice to pri" becomes "invoice" to Priya. The lead-in words ("from", "to", "with", "by") pick which side the person is on.
  - **Companies** by mail domain. "cedar" offers cedarpine.example. Free-mail providers and your own addresses are never offered.
  - **Recent searches** that start the same way.
  - **Refinements** from the results on screen, as with Gmail's chips: "From Mike Delgado 5", "With attachments 6", "In Inbox 4". Each appears only when it would narrow the list, never when it would empty it or change nothing.
- **Chips.** "from mike this year pdf" shows the chips "From Mike Delgado", "This year" and "Has PDF".
  - Removing a chip removes the words that made it (`nl.ts` `interpretDetailed` / `withoutPart`). The box never fills with operators.
  - "Just the words" goes back to a plain word search.
  - The old "Understood as `from:mike …`" line is gone, and so is Tab putting operator syntax into the box.
- **Dates.** A date-like word offers itself in words: *Use "february" as a date? Feb 1 – 28, 2026 ⇥*.
- **What was removed:**
  - The operator-name popup that opened on any word starting like an operator ("to", "in", "is", "has"…).
  - The "Tips" button and the empty-state list of operator examples.
  - The placeholder that listed operators. It now reads "Search mail: people, topics, files — or ask a question".
- **Kept for people who want operators:**
  - Typed operators still work, are highlighted, and still complete their values (`from:mi` lists people, now with avatars).
  - ⌘/ still opens the full reference.

### Keyboard

| Key | Does |
|---|---|
| ↑ ↓ | Move through results (the caret stays in the box: the WAI-ARIA combobox pattern) |
| ↵ | Open at the match |
| ⌘↵ | Open and keep the search |
| Tab | Take the first suggestion (or a completion, or the date offer); with none, jump to the next group. ⇧Tab goes back a group |
| ⌘↑ | The last search again (Spotlight: "Press the Up Arrow to recall a previous search") |
| Esc | Clear the query; the second press closes |
| ⌘S | Save the search |
| ⌘⇧↵ | Also search Gmail |
| ⌘/ | Operator reference |

### Empty, no results, and indexing

- **Before typing:**
  - Recent and saved searches, with Clear (Apple HIG: "provide a way for people to clear it").
  - The people you hear from most, one click from their mail. This replaces the old list of operator tips.
- **No results:**
  - **Did you mean.**
    - The words of mail Penguin has shown form an in-memory lexicon (`spelling.ts`). It learns from recent inbox threads and from every answer.
    - A word no mail contains is compared with its nearest neighbours by edit distance, counting a swap as one edit. It allows one edit up to five letters and two beyond that, and prefers a word with the same first letter.
    - The corrected query is run. When it finds mail by its words, its results are shown under "No mail has “leese renewel”. Showing results for lease renewal" (NN/g).
    - When meaning found something but no mail had the words, a "Did you mean lease renewal?" line sits above the results.
  - **Wider searches.** Each is run first and shown only if it finds mail, with its count. They are worded, not written as operators:
    - Any time.
    - Search all accounts, which removes the profile scope.
    - Include Trash and Spam.
    - Without "Last week".
    - Only "Mike Delgado", any time.
  - **Plain words about sync.** A note says when older mail is still downloading or is stored as headers only.
- **Indexing.** While `SearchResponse.semantic === "indexing"`, the footer carries one quiet line: *Getting smarter: 42% of your mail indexed*, with a small progress ring. Nothing is shown when the state is `ready` or `off`. A no-results page adds a sentence saying Penguin is still reading the mail.

## The contract it relies on

These fields are shared with the ranking backend (`types.ts`, mirrored in Rust):

- `SearchHit.matchedBy: ("words" | "meaning")[]`. An empty or missing value is treated as `["words"]`, so an older backend still lays out correctly.
- `SearchHit.passage?: string | null`. The best matching passage, plain text, at most 240 characters.
- `SearchResponse.semantic: "ready" | "indexing" | "off"`.
- `SearchResponse.semanticProgress?: number | null`, from 0 to 1.

In the mock (`src/lib/mock/semantic.ts`), hand-made groups of related words stand in for the embedding model. `?semantic=indexing|off|ready` (or localStorage `penguin.mock.semantic`) picks the state. `indexing` starts at 42% and climbs while the page is open.

## Measuring it

`scripts/bench-ui/search-paint.mjs` builds the mock bundle and types six real queries into the overlay, one key at a time every 160 ms, in headless Chromium. The queries are "invoice", "lease renewal", "from mike last week pdf", "has:pdf lease", "quarterly report" and "when did i last email priya".

It reports two timings:
- keydown to the typed character painted;
- keydown to that key's results painted.

For each it gives p50, p95 and the share under 50 ms and 100 ms (Superhuman's buckets). The results timing is split into four parts:
- key → search sent;
- sent → answered (the mock's 4 ms IPC hop plus its search);
- answered → DOM;
- DOM → paint.

Only the overlay's own search counts; facet counts, wider searches and spelling checks are left out. `--shots <dir>` saves screenshots of the key states in both themes.

**This change, 2026-09-26.** Measured on the shared Linux box (Chromium 153, load average 9–15 from other sessions). Before (`main`) and after were run alternately, three pairs of three passes each.

| p50 / p95, ms | before | after |
|---|---|---|
| Key → character painted | 20.5–22.9 / 35–47 | 21.7–27.2 / 40–43 |
| Key → results painted | 43.4–46.2 / 71–90 | 49.5–60.3 / 82–102 |
| Key → search sent | 9.6–10.7 | 10.1–11.8 |

Pooled at p50, results paint is about 45 ms before and 52 ms after: +7 ms, within one 16.7 ms frame. The gap comes from two places:
- About 3 ms is the mock's stand-in meaning matching, which runs in the page. In the app, meaning search runs in Rust.
- About 3 ms is paint, because the Top-results rows and the suggestion line are slightly more DOM.

Getting there meant fixing work on the typing path:
- The highlight mirror read `scrollLeft` (a forced layout of the whole overlay) on every render, including every search answer. It now reads it only when the text or caret moves. This was pre-existing and was the largest single cost in the CPU profile: busy time while typing fell from 10.8 s on `main` to 7.3 s for the same typing.
- Did-you-mean waits for a 300 ms pause. Its lexicon learns lazily and re-sorts only for new words.
- The suggestion line keeps its height while there's a query, so results don't jump by 30 px as suggestions come and go.
- The highlight pattern is compiled once per query, not per row.

The trade-off is that the reserved line is empty when there is nothing to suggest.

## Sources

- Apple, Human Interface Guidelines: [Searching](https://developer.apple.com/design/human-interface-guidelines/searching), [Search fields](https://developer.apple.com/design/human-interface-guidelines/search-fields) (read through Apple's JSON behind the pages)
- Apple Support: [Spotlight on Mac](https://support.apple.com/guide/mac-help/search-with-spotlight-mchlp1008/mac), [Search for emails in Mail on Mac](https://support.apple.com/guide/mail/search-for-emails-mlhlp1003/mac), [Search photos](https://support.apple.com/guide/photos/search-for-photos-and-videos-pht64de33e5a/mac), [About Spotlight indexing](https://support.apple.com/en-us/102321)
- Apple developer docs: [UISearchToken](https://developer.apple.com/documentation/uikit/uisearchtoken), [Performing a search operation](https://developer.apple.com/documentation/swiftui/performing-a-search-operation), [Suggesting search terms](https://developer.apple.com/documentation/swiftui/suggesting-search-terms), [NSTokenField](https://developer.apple.com/documentation/appkit/nstokenfield)
- Superhuman: [Delightful search](https://blog.superhuman.com/delightful-search-more-than-meets-the-eye/) (2017), [Built for speed: the 100ms rule](https://blog.superhuman.com/superhuman-is-built-for-speed/) (2022), [Performance metrics for blazingly fast web apps](https://blog.superhuman.com/performance-metrics-for-blazingly-fast-web-apps/) (2019)
- Linear: [Search](https://linear.app/docs/search), [Filters](https://linear.app/docs/filters), [Rebuilding delta sync](https://linear.app/now/rebuilding-delta-sync-read-path)
- Raycast: [Search bar manual](https://manual.raycast.com/search-bar), [List API](https://developers.raycast.com/api-reference/user-interface/list), [Store guidelines](https://developers.raycast.com/basics/prepare-an-extension-for-store)
- Google: [Gmail search chips](https://workspaceupdates.googleblog.com/2020/02/gmail-search-chips-ga.html) (2020), [Better search suggestions in Gmail](https://workspaceupdates.googleblog.com/2022/07/better-search-options-in-gmail.html) (2022), [Search in Gmail](https://support.google.com/mail/answer/6593), [Passage ranking](https://blog.google/products/search/search-on/) (2020), [RAIL](https://web.dev/articles/rail), [INP](https://web.dev/articles/inp), [CLS](https://web.dev/articles/cls); W3C [Layout Instability](https://wicg.github.io/layout-instability/)
- Nielsen Norman Group: [Response Time Limits](https://www.nngroup.com/articles/response-times-3-important-limits/), [Site Search Suggestions](https://www.nngroup.com/articles/site-search-suggestions/), [Enriched Site-Search Suggestions](https://www.nngroup.com/articles/enriched-site-search-suggestions/), [Search: Visible and Simple](https://www.nngroup.com/articles/search-visible-and-simple/), [Scoped Search](https://www.nngroup.com/articles/scoped-search/), ["No Results" pages](https://www.nngroup.com/articles/search-no-results-serp/), [Internal search engines](https://www.nngroup.com/articles/internal-website-search/), [User Intent Affects Filter Design](https://www.nngroup.com/articles/applying-filters/)
- W3C WAI-ARIA APG: [Combobox pattern](https://www.w3.org/WAI/ARIA/apg/patterns/combobox/)
- Slack: [AI features](https://slack.com/help/articles/25076892548883); Notion: [Search](https://www.notion.com/help/search); Shortwave: [Deep dive into its email AI](https://www.shortwave.com/blog/deep-dive-into-worlds-smartest-email-ai/) (2023); Algolia: [Improve performance](https://www.algolia.com/doc/guides/building-search-ui/going-further/improve-performance/js), [NeuralSearch and optimistic UI](https://www.algolia.com/blog/engineering/maintaining-speed-perception-with-neuralsearch-and-optimistic-ui) (2023)
- Research: Miller, "Response time in man-computer conversational transactions", AFIPS 1968 (Computer History Museum scan); Robertson, Card & Mackinlay, "Information Visualization Using 3D Interactive Animation", CACM 36(4), 1993; Doherty & Thadani, "The Economic Value of Rapid Response Time", IBM 1982 (transcription); Mackenzie et al., ["Exploring User Behavior in Email Re-Finding Tasks"](https://www.microsoft.com/en-us/research/uploads/prod/2019/02/Mackenzie-www19.pdf), WWW 2019; Elsweiler, Baillie & Ruthven, ["Exploring Memory in Email Re-finding"](https://epub.uni-regensburg.de/22682/1/Tois2008_exploring_email.pdf), TOIS 2008; Elsweiler, Harvey & Hacker, ["Understanding Re-finding Behavior in Naturalistic Email Interaction Logs"](https://epub.uni-regensburg.de/22696/1/sigir2011_email_logs.pdf), SIGIR 2011

Not fetched directly:
- The Superhuman Help Center (HTTP 403).
- Linear's command-menu page (404).
- Miller 1968 on the ACM Digital Library and the CHI '91 paper by Card, Robertson & Mackinlay (both 403). The mirror scan and the same authors' 1993 CACM paper were used instead.
- The original IBM scan of Doherty & Thadani (not found). A full transcription was used instead.
