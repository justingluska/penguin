# Building and developing Penguin

How to build Penguin from source, connect your accounts, find your way around the code, and send changes. Back to the [README](../README.md).

## Getting started

### Requirements

- **A Mac with Apple silicon.** Release builds target Apple silicon only; an Intel build may compile but isn't tested or published.
- **For thread summaries, Write with AI and suggested replies:** macOS 26 or later with Apple Intelligence turned on, and a build made with Xcode 26 or later. Everything else works without them.
- **To build:** Xcode (or its Command Line Tools), Rust via [rustup](https://rustup.rs), and Node.js 22.
- **Accounts:** your own Google Cloud OAuth client for Gmail, your own Microsoft Entra app registration for Outlook/Microsoft 365, or an app password for iCloud, Yahoo, AOL, Fastmail and other IMAP servers. Penguin ships no shared credentials.

### Build from source

```sh
git clone https://github.com/justingluska/penguin.git && cd penguin/apps/desktop
npm ci
npm run tauri build -- --bundles app
# The app is at target/release/bundle/macos/Penguin.app (or under src-tauri/target/)
```

Sign it with your own Apple Development identity so the Keychain access for your sign-ins survives rebuilds: `APPLE_SIGNING_IDENTITY="<your identity>" npm run tauri build -- --bundles app`. On a Mac, `scripts/install-local.sh` builds, installs and relaunches in one step.

Builds from source don't check for updates. Signed release builds get an update channel from the release workflow ([Releases and updates](#releases-and-updates)).

### Connect Gmail or Google Workspace

Penguin uses **your own** Google Cloud OAuth client, so no shared client and no Penguin server ever sees your tokens. Setup takes about ten minutes: create a project, enable the Gmail API, add the scopes, create a Desktop app client, and give its JSON to Penguin's onboarding. The step-by-step guide, including Workspace admin approval and the optional Google Calendar access, is **[google-setup.md](google-setup.md)**.

### Connect Outlook.com or Microsoft 365

Register your own app in [Microsoft Entra](https://entra.microsoft.com) (App registrations → New registration, "Public client/native" platform with redirect `http://localhost`, delegated Microsoft Graph permissions `offline_access User.Read Mail.ReadWrite Mail.Send MailboxSettings.Read`) and paste its Application (client) ID into Penguin's Microsoft sign-in. Work and school tenants often block user consent for mail permissions; Penguin then builds the admin-consent link to send to your IT admin. Details: [providers-and-onboarding.md](providers-and-onboarding.md#2-microsoft-outlookcom-hotmail-live-and-microsoft-365).

### Connect iCloud, Yahoo, AOL, Fastmail or any IMAP server

Type your address; Penguin detects the provider and its servers and walks you through creating an **app password** (iCloud, Yahoo, AOL and Fastmail don't offer third-party OAuth to self-built apps). Other servers take a password or app password, with the server settings found automatically or entered by hand. Passwords are stored in the macOS Keychain. Details: [providers-and-onboarding.md](providers-and-onboarding.md#3-yahooaol-icloud-fastmail-and-generic-imap).

### Try the UI without an account

```sh
cd apps/desktop && npm ci && npm run dev:mock    # http://localhost:1420, fictional mailbox
```

## Architecture

```
┌────────────────────────── Tauri 2 app (macOS) ───────────────────────────┐
│  React + TypeScript UI (apps/desktop/src)                                 │
│    talks only through src/lib/api.ts ── invoke() / events ──┐             │
│                                                             ▼             │
│  src-tauri: thin command glue, background tasks, penguin-cli + MCP        │
│      │ every mailbox call goes through the provider seam                  │
│      ▼                                                                    │
│  penguin-provider ── MailProvider / Backend, compose, outbox, snooze      │
│      │ implemented by                                                     │
│      ├── penguin-gmail   Gmail REST, Google OAuth, Google Calendar        │
│      ├── penguin-graph   Outlook.com / Microsoft 365 over Microsoft Graph │
│      └── penguin-imap    IMAP + SMTP (iCloud, Yahoo, Fastmail, any server)│
│                                                                           │
│  penguin-core      SQLite (WAL) + FTS5 store, query parser, Ask, smart    │
│                    views, extraction. No networking, no Tauri.            │
│  penguin-semantic  embeddings (ONNX Runtime) and the int4/int8 vector     │
│                    index                                                  │
│  penguin-render    email HTML sanitizer, tracker and link cleaning        │
│  PenguinAI (Swift) bridge to Apple's Foundation Models (macOS 26+)        │
└──────┬────────────────────────────────────────────────────────────────────┘
       │ HTTPS / IMAPS / SMTPS, straight from your Mac
       ▼
   Gmail · Microsoft Graph · IMAP/SMTP servers        (no Penguin server)
```

| Path | What |
|---|---|
| `crates/penguin-core` | Types, the SQLite + FTS5 store, the search query parser, Ask, smart views and fact extraction. No networking, no Tauri, so it's tested anywhere. |
| `crates/penguin-provider` | The seam every mail provider implements (`MailProvider` / `Backend`), plus outgoing MIME, send later, reminders, snooze, credential vaults and a conformance test suite. |
| `crates/penguin-gmail` | OAuth (loopback + PKCE), Keychain tokens, the Gmail REST client and sync engine, Google Calendar. |
| `crates/penguin-graph` | Outlook.com, Hotmail and Microsoft 365 through Microsoft Graph (per-folder delta, categories, drafts, send, meeting replies). |
| `crates/penguin-imap` | IMAP and SMTP, with its own wire layer over rustls. |
| `crates/penguin-render` | The email HTML sanitizer (ammonia/html5ever), tracker and link-tracking removal, unsubscribe parsing. |
| `crates/penguin-semantic` | Chunking, EmbeddingGemma through ONNX Runtime, the quantized vector index. |
| `crates/penguin-eval` | The search relevance harness. Dev only; the app doesn't ship it. |
| `apps/desktop/src-tauri` | The Tauri 2 app: command glue, background tasks (sync, indexing, extraction, outbox, rules), `penguin-cli` and the MCP server, the Swift bridge. |
| `apps/desktop/src` | The React + Vite UI and the in-browser mock backend (`src/lib/mock`). |
| `design/` | Mockups and the `penguin.css` design system. |

- **Local-first store.** One SQLite database in WAL mode: one writer connection for sync and your actions, a pool of read-only connections for the UI, so a backfill never blocks a list or a search. Full text lives in a contentless FTS5 index; bodies are stored zstd-compressed.
- **The provider seam.** The UI and the store don't know which provider a message came from. Adding a provider means implementing `penguin-provider`'s traits and passing its conformance suite ([PROVIDERS-IMPL.md](PROVIDERS-IMPL.md)).
- **Hostile HTML never runs.** Email HTML is sanitized in Rust with an allowlist, remote content is rewritten or held back, and the result is shown in a sandboxed iframe without scripts, under a strict content security policy. [SECURITY.md](SECURITY.md) has the four layers.
- **Actions are optimistic.** Archive, label, star, snooze and read state change the list and every count in the same frame as the key; the provider call runs behind it, and a refused call puts things back and says why.

[ARCHITECTURE.md](ARCHITECTURE.md) has the command contract and the reasoning behind the main decisions.

## Contributing and development

Issues and pull requests are welcome. Please read [ARCHITECTURE.md](ARCHITECTURE.md) first: it has the command contract and which module owns what.

```sh
# Rust: check and test the workspace
cargo check --workspace
cargo test --workspace

# The UI alone on the mock backend (no account needed): http://localhost:1420
cd apps/desktop && npm ci && npm run dev:mock

# The full app
cd apps/desktop && npm run tauri dev

# UI unit tests and the type check
cd apps/desktop && npm test && npx tsc --noEmit

# Search quality: run the judged query set, then compare two runs
cargo run -p penguin-eval --release -- run
cargo run -p penguin-eval --release -- diff crates/penguin-eval/baselines/keyword.json target/search-eval/keyword.json

# Performance: benchmarks on 10k/100k/300k synthetic mailboxes, regenerates docs/PERFORMANCE.md
scripts/bench.sh
```

Ground rules:

- **Fixtures, mock data and screenshots use fictional people and `.example` domains only.** Never commit OAuth client JSON, tokens, `.env` files or mailbox data, and never log tokens or message bodies.
- **Judge every ranking change** with `penguin-eval` on the same day and build, and keep identifier, operator and name queries from regressing ([SEARCH-EVAL.md](SEARCH-EVAL.md)).
- **Interactions read local state only.** Never block the UI on the network.
- **Email HTML** is rendered only through `penguin-render` and the sandboxed iframe.
- Keep `types.rs` and `types.ts` in lockstep.

### Releases and updates

The in-app updater is off unless a build is given an update channel. `.github/workflows/release.yml` builds, signs, notarizes and publishes a release on every push to `main`, passing the channel to the build with `tauri build --config`. Each release also gets a signed, notarized `Penguin.dmg` for first installs; version tags publish it as a GitHub release too. It needs these repository variables and secrets (listed at the top of the workflow):

- Variables: `PENGUIN_UPDATE_URL` (public base URL that serves `latest.json`), `PENGUIN_UPDATER_PUBKEY` (from `npx tauri signer generate`), `PENGUIN_UPDATE_BUCKET`.
- Secrets: the Developer ID certificate, an App Store Connect API key for notarization, the updater private key, and an S3-compatible upload key.

Until all of them are set, the workflow skips the macOS build with a warning.

## License

Penguin is **source-available** under the [PolyForm Shield License 1.0.0](../LICENSE).

In plain words:

- **You can** use Penguin, personally or at work, read and change the code, fix bugs, send improvements back, and build other things with it that don't compete with it.
- **You can't** sell Penguin, offer it as a paid or hosted service, or release your own version of it as a competing email app, paid or free.
- If the project is ever discontinued, the license lets others carry it on.

The license text is the only thing that's binding; this summary is here to help. It isn't an OSI-approved open-source license, because it restricts competing use.

The name "Penguin" and the Penguin icon aren't covered by the license: please don't use them for your own product.

Contributions are welcome under the same license. By sending a pull request you agree that your contribution is licensed under it, and that the maintainer may relicense the project, including your contribution, in the future.
