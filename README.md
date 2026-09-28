<p align="center">
  <img src="apps/desktop/src-tauri/icons/128x128@2x.png" width="112" alt="Penguin icon">
</p>

<h1 align="center">Penguin</h1>

<p align="center">
  <b>A fast, private, keyboard-first email client for the Mac.</b><br>
  Your whole mailbox lives on your Mac, so search, triage and AI never wait on the network.
</p>

<p align="center">
  <img alt="macOS, Apple silicon" src="https://img.shields.io/badge/macOS-Apple%20silicon-000000?logo=apple&logoColor=white">
  <img alt="Local-first, no servers" src="https://img.shields.io/badge/local--first-no%20servers-2ea44f">
  <img alt="On-device AI" src="https://img.shields.io/badge/AI-on--device-8a63d2">
  <img alt="Built with Rust and Tauri 2" src="https://img.shields.io/badge/Rust-Tauri%202-dea584?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="License: PolyForm Shield 1.0.0" src="https://img.shields.io/badge/license-PolyForm%20Shield-blue"></a>
  <img alt="Status: beta" src="https://img.shields.io/badge/status-beta-orange">
</p>

<p align="center">
  <a href="https://github.com/justingluska/penguin/releases/latest/download/Penguin.dmg"><b>Download for Mac</b></a> ·
  <a href="docs/FEATURES.md">Features</a> ·
  <a href="docs/DEVELOPMENT.md">Build from source</a> ·
  <a href="docs/PERFORMANCE.md">Performance</a> ·
  <a href="docs/PRIVACY.md">Privacy</a> ·
  <a href="#docs">Docs</a>
</p>

<img alt="Penguin's inbox with Split Inbox tabs (Important, Calendar, News, Other), a unified list across four accounts, and a conversation with three attachments open in the reading pane" src="docs/images/readme/inbox-light.jpg">

Gmail and Google Workspace come first; Outlook.com, Microsoft 365, iCloud, Yahoo, Fastmail and any IMAP server work too. There are no Penguin servers: the app talks to your mail provider directly.

> **Beta.** [Download Penguin for Mac](https://github.com/justingluska/penguin/releases/latest/download/Penguin.dmg) (Apple silicon). Gmail needs your own Google OAuth client: about ten minutes, [step by step](docs/google-setup.md).

## Why Penguin

- ⚡ **Fast.** Every key press is answered from a local database. Searching 300,000 messages takes a few milliseconds.
- 🔍 **Search that finds it.** Gmail's operators, typo tolerance, plain-English dates, and search by meaning.
- 💬 **Ask your inbox.** "When is my flight to Lisbon?" Exact answers from your mail, with the emails they came from.
- 🧠 **Smart, on your Mac.** Summaries, Write with AI and search by meaning run on local models. No API key, nothing sent anywhere.
- 🔒 **Private by default.** Tracking pixels removed, remote images held, hostile email HTML sanitized and sandboxed.
- ⌨️ **Keyboard-first.** Gmail-style keys, a ⌘K palette with every command, and a coach that teaches you the keys.

## Search that finds it

Press `/` and type. Results update on every keystroke across all your accounts, with facets, matching attachments and the people behind them. It forgives typos, word forms and accents, and a local embedding model finds mail that says the same thing in other words: "plane tickets to portugal" finds the Lisbon boarding passes.

<p>
  <img width="49%" alt="Search for 'lease from:mike has:pdf': the operators become removable chips, facets on the left, top results with highlighted matches, the matching PDF attachments, threads and people" src="docs/images/readme/search.jpg">
  <img width="49%" alt="Search by meaning: 'plane tickets to portugal' has no keyword match, and the closest match is 'Your trip to Lisbon — boarding passes'" src="docs/images/readme/search-meaning.jpg">
</p>

Scored on 570 judged queries, keyword plus meaning reaches an nDCG@10 of 0.951. → [Search in depth](docs/FEATURES.md#search) · [operators](docs/SEARCH.md) · [how it's measured](docs/SEARCH-EVAL.md)

## Ask your inbox

Type a question into search. Penguin computes the answer from your mail (flights, stays, receipts, parcels, bills, people) and shows the emails it came from. Counts and totals read every matching email, never a sample, and no language model writes the answer.

<p>
  <img width="49%" alt="Ask answering 'when is my flight to Lisbon?': flight AU 238 from SFO to LIS on Sat, Oct 3 at 7:05 PM, the return leg, confirmation code RXJ34P, marked Exact, answered in 9 ms on this Mac" src="docs/images/readme/ask-flight.jpg">
  <img width="49%" alt="Ask answering 'how much did I spend at Ledgerly this year?': $376.00 across 5 Ledgerly receipts, what was left out, the largest and average, and the receipts it added" src="docs/images/readme/ask-spend.jpg">
</p>

→ [How Ask works](docs/ASK.md)

## Smart, with AI that stays on your Mac

Thread summaries (with anything asked of you and its deadline), Write with AI and suggested replies run on Apple's on-device model. Search by meaning uses a small embedding model that runs locally. No API key, no server, and nothing runs until you ask.

<p>
  <img width="49%" alt="A thread summary card: the gist, key points each linked to its message, and 'Asked of you: Can you own the Friday retro block?' with a 'due Friday' chip" src="docs/images/readme/summary.jpg">
  <img width="49%" alt="Write with AI in a reply: the instruction became a drafted reply shown as a suggestion with Try again, Discard and Accept" src="docs/images/readme/write-with-ai.jpg">
</p>

→ [On-device AI](docs/FEATURES.md#on-device-ai) · [summaries](docs/SUMMARIES.md) · [search by meaning](docs/SEMANTIC.md)

## Fast

<!-- perf:start -->
| Search 300,000 messages | Search as you type | Open a conversation | Answer a question |
|:---:|:---:|:---:|:---:|
| **5.6 ms** | **5.0 ms** a keystroke | **0.32 ms** | **8.2 ms** |

Medians on a synthetic 300,000-message mailbox on an Apple M5 Pro (macOS 26.6.2, 18 cores, 64 GiB RAM), release build, commit 9d41ddcd5c7c. How it's measured, every number and why it's fast: [docs/PERFORMANCE.md](docs/PERFORMANCE.md).
<!-- perf:end -->

Archive, label, star and snooze change the screen in the same frame as the key; the server catches up behind it.

## Built for inbox zero

Split Inbox, Get to zero, Reply Later, snooze, follow-up reminders, snippets, instant replies, send later and undo send.

<p>
  <img width="49%" alt="The Get to zero dialog: 'Older than two weeks' selected, Keep starred checked, '126 conversations will be archived, 11 kept'" src="docs/images/readme/get-to-zero.jpg">
  <img width="49%" alt="The ⌘K command palette: 'Type a command, a person, or search mail…' with suggested commands and Go to entries" src="docs/images/readme/command-palette.jpg">
</p>

Plus smart views for receipts, travel, packages and bills, calendar invites you can answer in the thread, rules, profiles, and a read-only MCP server and CLI for agents. → [Every feature](docs/FEATURES.md)

## Private by design

- Your mail, search index and AI stay on your Mac. Tokens and passwords live in the macOS Keychain.
- Tracking pixels are blocked, remote images wait until you ask, and link tracking can be stripped.
- Email HTML is sanitized in Rust and shown in a sandboxed frame with no scripts.

→ [Privacy](docs/PRIVACY.md) · [Security and reporting a vulnerability](docs/SECURITY.md)

## Getting started

1. **[Download Penguin.dmg](https://github.com/justingluska/penguin/releases/latest/download/Penguin.dmg)**, open it and drag Penguin to Applications. It's signed and notarized by Apple, and it keeps itself up to date.
2. **Add your accounts.** Gmail and Google Workspace use your own Google OAuth client, so no shared client and no Penguin server ever sees your tokens: about ten minutes, [step by step](docs/google-setup.md). Outlook uses your own Microsoft app registration, and iCloud, Yahoo, Fastmail and other IMAP servers use an app password ([details](docs/providers-and-onboarding.md)).

Needs a Mac with Apple silicon. Summaries and Write with AI need macOS 26 with Apple Intelligence; everything else works without them.

**Build from source** or try the UI on a fictional mailbox with no account:

```sh
git clone https://github.com/justingluska/penguin.git && cd penguin/apps/desktop
npm ci && npm run dev:mock      # http://localhost:1420
npm run tauri build -- --bundles app
```

Requirements and details: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Docs

| | |
|---|---|
| [Features](docs/FEATURES.md) | The full tour, how Penguin compares, known limitations |
| [Development](docs/DEVELOPMENT.md) | Build, connect accounts, architecture, contributing, releases |
| [Search](docs/SEARCH.md) · [Ask](docs/ASK.md) | The query language and how questions are answered |
| [Performance](docs/PERFORMANCE.md) | Every benchmark and why it's fast |
| [Privacy](docs/PRIVACY.md) · [Security](docs/SECURITY.md) | Tracking protection, threat model, reporting a vulnerability |
| [CLI and MCP](docs/CLI.md) | `penguin-cli` and the MCP server for agents |

## License

Source-available under the [PolyForm Shield License 1.0.0](LICENSE): use it, change it and send improvements back, but don't sell it or ship it as a competing email app. The name "Penguin" and its icon aren't covered by the license. Full terms and contribution notes: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#license).
