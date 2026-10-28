# Cross-device state (Mac ⇄ iPhone)

Status, 2026-10-13:
- **Decided:** the design below.
- **Built and tested (Rust):**
  - the `penguin-sync` crate: record model, clocks, merge rules, transport and local-store seams, in-memory server, settings mapping, status;
  - penguin-core's records for snoozes and Follow up dismissals: emitted, merged and applied in the store;
  - send later and reminders (the outbox): records, the one-sender rule with claims, applied in the store and asked by penguin-provider's scheduler before every send (`penguin_sync::outbox`, `store_outbox.rs`);
  - the wider settings set (appearance, swipes, Split Inbox, sender photos) and "keep on this device" (`keepHere`);
  - the snooze wake gate in penguin-provider;
  - `penguin-core/src/cloud.rs` (`CloudHost`): the device side of the CloudKit transport, the same on both devices; tested end to end with two devices through a fake shim (`cloud_tests.rs`).
- **Built, compiled and signed on CI; not yet run against iCloud by a person:**
  - the CKSyncEngine shim, one Swift file for both apps (`apps/desktop/src-tauri/swift/PenguinAI/Sources/PenguinAI/CloudSync.swift`);
  - the Mac: C bridge (`CloudSyncBridge.swift`), `src-tauri/src/cloud.rs`, settings wiring, Settings → Accounts → Sync with iCloud, entitlements and the embedded Developer ID profile (release.yml);
  - the iPhone: `penguin-ffi/src/cloud.rs`, `ios/Penguin/Sync/`, entitlements in `ios/project.yml`, Settings → Sync with iCloud.
- **Not built:** snippet files across devices, tombstone GC.

Tasks: PEN-30 (research and design), PEN-31 (shared core), PEN-32 (migration and Settings).

## The problem

Penguin has no server. Several kinds of state exist only in the Mac's SQLite database or in `settings.json`:
- snoozes, with their wake times;
- Follow up dismissals;
- send-later schedules and "remind me" reminders (the outbox);
- snippets, instant replies, signatures, splits, smart views, profiles and other settings.

The native iPhone app (branch `ios-prototype`, Rust core over UniFFI) needs the same state. The reference user has about 10 Gmail and Workspace accounts, and the codebase also has IMAP and Microsoft 365 providers.

Reply Later is already a real label (a Gmail label, an IMAP folder, a Microsoft category), so it syncs through the mailbox. Nothing here changes that.

## Recommendation: CloudKit private database, merged by a shared Rust core

- **Where the state lives:** each user's records go in one custom zone of the app's CloudKit container, in that user's **private database**. The store is a single one shared by every account and every provider.
- **What moves the records:** CKSyncEngine, the sync engine Apple ships for this job, behind a thin Swift shim on each device. On the Mac it is linked the same way `swift/PenguinAI` already is; on the iPhone it is native.
- **What decides conflicts:** the Rust core (`crates/penguin-sync` plus `penguin-core/src/store_sync.rs`). The Swift shim moves records and does not interpret them.
- **What CloudKit sees:** each record is an opaque JSON payload in one `encryptedValues` field. Its name is a hash.

Peer sync over the Mac↔iPhone pairing channel can be added later as a second transport. It suits a LAN fast path or a Mac-only household with no iCloud. It would use the same records and the same merge, behind the same `Transport` trait. It is a supplement, not the primary path: it only works while both devices are awake on the same network.

### Why CloudKit

1. **It is one store for every account and every provider.** CloudKit is keyed to the person's Apple ID, not to a mail account.
   - Drive's `appDataFolder` belongs to one Google account. With 10 accounts, one would have to be chosen as "home".
   - IMAP-only and Microsoft-only users would have no home at all.
   - A Workspace admin can switch off Drive SDK access, which breaks third-party Drive API apps ([Google admin help](https://support.google.com/a/answer/6105699)).
2. **It does the work a server would otherwise do, at no cost and with no Penguin server.** CKSyncEngine:
   - schedules pushes and fetches, persists its own state and batches records (up to 250 per request);
   - retries offline, rate-limit and account-unavailable errors by itself;
   - subscribes to silent pushes, so the phone hears about a change in seconds rather than by polling;
   - waits quietly when no one is signed in to iCloud and starts after sign-in.
   - Sources: [CKSyncEngine](https://developer.apple.com/documentation/cloudkit/cksyncengine-5sie5). Drive change notifications, by contrast, need a public HTTPS webhook ([Drive push](https://developers.google.com/workspace/drive/api/guides/push)), so without a server Drive means polling.
3. **It provides real compare-and-swap, which correct merging needs.** Every save is checked against `recordChangeTag`. A save that loses comes back as `serverRecordChanged` with the server's record, which we merge into and save again ([serverRecordChanged](https://developer.apple.com/documentation/cloudkit/ckerror/code/serverrecordchanged), [recordChangeTag](https://developer.apple.com/documentation/cloudkit/ckrecord/recordchangetag)). Drive v3 files have no `etag`, only a `version` counter, and no documented `If-Match` on `files.update` ([files resource](https://developers.google.com/workspace/drive/api/reference/rest/v3/files)).
4. **Privacy is strong and needs no setup.**
   - The private database is readable only by the user.
   - `encryptedValues` fields are encrypted on the device. With Advanced Data Protection, only the user holds the keys ([encryptedValues](https://developer.apple.com/documentation/cloudkit/ckrecord/encryptedvalues), [Encrypting user data](https://developer.apple.com/documentation/cloudkit/encrypting-user-data)).
   - Storage counts against the user's own iCloud quota ([quotaExceeded](https://developer.apple.com/documentation/cloudkit/ckerror/code/quotaexceeded)), and our records are kilobytes.
5. **The Mac build can carry it.** Apple lists "iCloud: CloudKit" and "Push notifications" as available to **Developer ID** apps, not only Mac App Store apps ([supported macOS capabilities](https://developer.apple.com/help/account/reference/supported-capabilities-macos)). Tauri 2 can embed the Developer ID provisioning profile with `bundle.macOS.files` and set the entitlements with `bundle.macOS.entitlements` ([Tauri macOS bundle](https://v2.tauri.app/distribute/macos-application-bundle/), [Tauri App Store guide](https://v2.tauri.app/distribute/app-store/)). Penguin ships only for macOS and iOS, both under one Apple team, so being Apple-only costs nothing.

The costs, stated plainly:
- A Swift shim is needed on each side.
- Users must be signed in to iCloud. When they aren't, the app works as it does today, with state on each device only.
- Every self-build needs its own CloudKit container. See "Open-source builds" below.

### Rejected options

| Option | Why not |
|---|---|
| **Google Drive `appDataFolder`** (`drive.appdata`) | The scope is non-sensitive, so it adds no OAuth verification burden ([Drive scopes](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)). Even so: (1) it is tied to one Google account, so one of ~10 has to be "home", and it does not exist at all for IMAP- or Microsoft-only users. (2) It has no compare-and-swap, so concurrent writes race; working around that means one file per device and a full merge on every read. (3) There is no push without a server, so the phone must poll `changes.list` (100 quota units a call; cheap, but it costs battery and adds latency). (4) Google calls the folder "not a durable store": users can delete it, and uninstalling the app from Drive deletes it ([appdata guide](https://developers.google.com/workspace/drive/api/guides/appdata)). (5) Admins can block the Drive API for Workspace users. (6) Every user has to enable the Drive API in their own Cloud project and re-consent every account. Worth keeping in reserve as a second transport for a future non-Apple client. |
| **State in the mailbox** (labels for snooze, JSON in a hidden draft or message, IMAP METADATA) | (1) Labels cannot carry a wake time without a label per time, and Gmail caps a mailbox at 10,000 labels ([Label schema](https://googleapis.dev/nodejs/googleapis/latest/gmail/interfaces/Schema$Label.html)). (2) A JSON message added with `messages.insert` shows in All Mail and in search, and Workspace Vault keeps it ([messages.insert](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/insert)). (3) Updating a message is delete-and-insert, with no compare-and-swap. (4) It is per account, so settings have no home. (5) IMAP METADATA (RFC 5464) is not advertised by Gmail, Outlook.com or Fastmail (CAPABILITY checked before login, 2026-10-11; see also [Gmail IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)). (6) The JMAP/IMAP snooze extension draft has expired ([draft-ietf-extra-email-snooze](https://datatracker.ietf.org/doc/draft-ietf-extra-email-snooze/)). Reply Later stays a label because it carries no time and other clients should show it. |
| **CloudKit Web Services / CloudKit JS from Rust** | Private-database access needs a web sign-in token that is single-use and rotates on every response, and server-to-server keys only reach the public database ([CloudKit Web Services](https://developer.apple.com/library/archive/documentation/DataManagement/Conceptual/CloudKitWebServicesReference/SettingUpWebServices.html)). The native framework through a Swift shim is simpler and gets push. |
| **Peer-to-peer only** (Mac↔iPhone pairing channel) | It works only while both devices are awake and on the same network. The cases that matter most fail: snoozing on the phone on the train while the Mac sleeps, or the Mac waking a snooze the phone made. It stays a possible second transport. |
| **A Penguin server** | Out of scope by design (docs/ARCHITECTURE.md: "There is no Penguin server"). |

How other clients handle this, for context:
- **Gmail:** its own snooze is not exposed through the API ([issue 109952618](https://issuetracker.google.com/issues/109952618)).
- **Mimestream:** snoozes "locally within the app" ([Mimestream help](https://mimestream.com/help/user-guide/snoozing)).
- **Spark:** syncs snoozes across devices and keeps scheduled sends on its own servers ([Spark send later](https://support.readdle.com/spark/sending-emails/schedule-an-email-to-send-later)).
- **Fastmail:** snoozes on its server, visible only in its own apps ([Fastmail help](https://www.fastmail.help/hc/en-us/articles/360058753634)).
- **Outlook:** snoozes on the server, with no API to set the time ([Graph mailFolder](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder)).
- **Thunderbird:** has no snooze.

Penguin needs cross-device state without a server of its own, and CloudKit is the user's own server.

## Data model

Code: `crates/penguin-sync/src/record.rs`.

A **record** is one piece of synced state:

```
Record { kind, key, gen: Hlc, phase: u8, at: Hlc, body: Option<JSON> }
```

- `kind` + `key` name it. Thread state uses `thread_key(account, thread)`, which is the lowercased account address, U+001F, then the provider's thread id.
- The record name on the server is `<kind>-<first 20 bytes of sha256(kind, key) in hex>`. Addresses and thread ids never appear in names, which CloudKit does not encrypt.
- `body: None` is a tombstone: the state was deleted.

| Kind | Key | Body | Status |
|---|---|---|---|
| `snooze` | account + thread | `{state: snoozed\|ended\|woken, wakeAt, snoozedAt, endedAt?}` | wired (store_snooze.rs) |
| `followUpDismissal` | account + thread | `{sentAt}` | wired (store_triage.rs) |
| `setting` | top-level settings key | the key's JSON value | modelled (`settings.rs`); app wiring to do |
| `snippet`, `signature` | item `id` | the item | wired (both apps' settings files) |
| `reminder` | account + reminder id | `{accountId, threadId, sentAt, remindAt, createdAt, sentMessageId?, outcome?}` | wired (store_outbox.rs) |
| `scheduledSend` | account + schedule id | `{accountId, draftId, sendAt, createdAt, remindAfterMs?, owner, claimedBy?, claimedAt?, outcome?}` | wired (store_outbox.rs; the iPhone's own outbox publishes its schedules) |

**What syncs and what stays on each device.** Settings are synced per key (`penguin_sync::settings::SYNCED_SETTINGS`); lists people grow on both devices (snippets, signatures) per item. A device can keep any synced key to itself (`keepHere`, below).

| Setting (settings.json key) | | Why |
|---|---|---|
| Signatures (`signatures`, per item), defaults per account, insertion, separator | **synced** | the iPhone's own signatures (PEN-88) use the same key and item shape (`id`, `name`, `html`); the Mac's extra fields survive a phone edit (overlay) |
| Snippets (`snippets`, per item) | **synced**, text and fields; **files stay on the Mac** | a snippet's files are local files by hash (up to 25 MB each); a phone edit keeps them (overlay). See "Gaps" |
| Instant replies, Write with AI | **synced** | |
| Profiles, hidden from All, account order | **synced** | each device keeps the others' accounts in them (`keep_other_devices_accounts`) |
| Inbox splits, Split Inbox on/off (`inboxTabs`), Get to zero, its button, the zero celebration | **synced** | |
| Smart views | **synced** | the iPhone keeps a subset (overlay) |
| Privacy: remote images, trusted image senders, tracking pixels, link tracking, read receipts | **synced** | |
| Follow up days, undo-send delay, send-later morning hour, lock reply account, unsubscribe button | **synced** | |
| New-mail notifications (on/off, per account, known senders only) | **synced** | whether the system lets Penguin notify is each device's own |
| Swipes (`swipeRight`, `swipeLeft`, `swipeLeftLong`) | **synced** | the iPhone's own Reply Later and Snooze read as the Mac's default there; each device keeps its own defaults until one is set |
| Appearance: theme, accent, match accent, sidebar theme, dark shade, corners, list style, dark email bodies, conversation order | **synced** | the theme can be kept per device: "Only on this Mac / iPhone" (`keepHere: ["theme"]`), e.g. the phone on System while the Mac stays Dark |
| Sender photos (sources) and where they show | **synced** | |
| Rules | **the rules feature's own record** | the Mac publishes its rules for the iPhone as one read-only record and decides what runs where (PEN-89, penguin-rules `mirror.rs`); not a setting |
| Labels' colors and hidden | **through the provider** | Gmail keeps them on the label (`color`, `labelListVisibility`), so they reach every device with mail sync; no record |
| Density, fonts and sizes, sidebar text size, Floe mode, shortcut hints and coach, spelling and grammar | **this device** | they depend on the screen or the keyboard |
| Keep mail for (sync window, older mail), background refresh, the badge, the Gmail rate budget | **this device** | what this device downloads and runs |
| Agents/MCP, on-device models (semantic search, summaries, Ask), calendar sync | **this device** | what this machine runs |
| Window state, welcome, pending setups, "You" (`me`, its photo a local file), `keepHere` itself | **this device** | one-off or local state |
| The notification permission | **this device** | the system's, per device |

**`keepHere`** (both apps' settings files, never synced): synced keys this device keeps to itself. A listed key is neither recorded when it changes nor replaced by another device's value (`settings::changes`, `settings::apply`). Taking a key off the list takes the synced value again instead of sending this device's (`settings::unkept`, `CloudHost::settings_write`). Settings offers it for the theme.

**Never synced, in any form:**
- OAuth or refresh tokens, IMAP passwords, client secrets or Keychain items;
- message bodies, subjects, snippets or attachments;
- the mailbox index;
- logs;
- `me.photo`.

Snippet attachments are local files by hash, so the snippet syncs without its files (see "Gaps").

## Conflict rules

All of these are in `merge()`.

Each record carries three stamps from a **hybrid logical clock**:
- **`gen`:** when the user intent behind this version was made. Every user action (snooze, unsnooze, re-snooze, dismiss, change a setting) starts a new generation.
- **`phase`:** how far that intent has progressed. System events only move it forward. For a snooze: 0 waiting, 1 ended by itself, 2 woken.
- **`at`:** when this exact version was written. It is the last tie-break.

**Merge keeps the larger `(gen, phase, at)`.** That is a total order, so the merge is commutative, associative and idempotent: devices converge whatever order records arrive in, and a test folds all 720 orderings of six versions to the same winner.

What the rule does in practice:
- **Last writer wins** for plain values (settings, dismissals): each edit is a new generation.
- **Delete vs edit:** a tombstone is a generation like any other, so the later intent wins either way.
- **Concurrent snoozes:** the later snooze wins.
- **Re-snooze vs wake:** a re-snooze made on an offline phone before the Mac woke the old snooze still wins, because it is a newer intent than the snooze the Mac woke.
- **Unsnooze vs wake:** the unsnooze wins; it is newer.
- **Wake vs stale copy:** a woken snooze can't be undone by a device that hadn't heard yet, because same generation means the higher phase wins.

**Clocks** (`hlc.rs`): HLC (Kulkarni et al., [Logical Physical Clocks, 2014](https://www.cse.buffalo.edu/~demirbas/publications/hlc.pdf)). Stamps never go backwards, and a device stamps after everything it has seen, so a Mac with a slow clock still wins with an edit it made after seeing the phone's.

**Clock skew:** every stamp is clamped, for ordering and for observing, to the server's `modificationDate` + 10 minutes. That date is identical on every device. So a device whose clock is a day fast loses its head start once its record is stored, and it can't drag the other devices' clocks forward. Tested in `a_fast_clock_loses_its_head_start_once_stored` and `a_broken_future_clock_drags_others_only_to_the_server_time`.

**A snooze wakes once.** Waking has two steps, and only the second is shared:
1. **Local wake:** at `wakeAt` the device puts the thread back in its own inbox, even offline, and remembers what it changed (`wake_undo`). No record is written.
2. **Commit:** the device tells the provider (`threads.modify` +INBOX +UNREAD) and writes the `woken` phase. It may only commit after a pull that completed after the wake time (`Store::snooze_wake_committable`; `PULL_WAIT_RETRY_MS`). The app asks the transport to fetch when a wake is due.
   - If that pull brings a re-snooze or an unsnooze, the local wake is undone (`revert_wake`) and the provider never hears of it.
   - If it brings `woken` from another device, this device only follows: it wakes locally, marks the wake as pushed and never calls the provider.
   - Two devices that are both fresh at the same second can both commit. Both send the same idempotent label change before the user could act, and their records merge.
   - With sync off the gate is always open, so a single Mac behaves exactly as before.

**Ended by itself** (a reply brought the thread back, or it was trashed, marked spam, deleted, or got a 404 at wake time): the device writes `ended` in the **same** generation. That can't cancel a newer snooze made elsewhere. `Store::end_snooze` replaces the old `unsnooze` on a 404 for this reason.

**A snooze from another device, before this device has seen the archive:**
- The row is `remote_pending`. A thread still in this device's inbox is not "back in the inbox" until the archive has been seen here once.
- New mail newer than `snoozedAt` still ends it.
- A thread trashed here is not projected.

**Accounts:**
- Records for an account this device doesn't have are kept but not projected. They apply when the account is added.
- Removing an account here deletes only the local rows, never the records, so another device keeps its snoozes.

**Tombstones** stay on the server. A future GC may delete those older than 60 days. A device whose change feed reports a deletion forgets the record unless it has unsent edits, and those are pushed again as a new record (tested).

### The outbox: exactly one device sends a scheduled message

Code: `crates/penguin-sync/src/outbox.rs` (the rule, pure), `penguin-core/src/store_outbox.rs` (records, projection, `scheduled_send_turn`, `reminder_turn`), `penguin-provider/src/outbox.rs` (asks before every send).

A double send is the one failure that can't be undone, so a schedule has an **owner**: the device that scheduled it.
- **On time:** the owner sends it, without asking anyone, for 10 minutes after it is due (`OWNER_FREE_MS`).
- **Elsewhere:** every other device shows it ("Scheduled on your other device"), can cancel it (a tombstone, a new generation) or replace it (a new schedule, owned by the device that made it), and is not due for it.
- **Taking over:** once it is 15 minutes overdue (`TAKEOVER_AFTER_MS`) and still not sent, another device may take it over. Taking over is a **claim**: the record's next phase, naming the claimer. A device sends a claimed schedule only after the server has stored **its** claim.
  - Two devices claiming at once: CloudKit stores one save and returns the other as `serverRecordChanged`. In `merge`, a claim the server stored beats a concurrent claim it didn't, whatever the clocks say (`outbox::claim_winner`), so the loser takes the stored claim and waits. Tested both ways round, through the in-memory server and two real Stores.
  - A late owner claims too, so the owner and a device taking over go through the same gate. The five minutes between the two windows absorb clock skew and a send in flight.
  - A claim not followed by a send in 30 minutes (`CLAIM_STALE_MS`) is abandoned: another device may claim again one phase up. An owner whose own claim never reached the server sends anyway then.
- **Only where a second send is caught:** taking over needs a provider whose send consumes a server draft (Gmail's `drafts.send`; a second send of the same draft is a 404) and the draft's id. IMAP and SMTP mail is only ever sent by its owner, which then never waits for a claim. A schedule that never reached the server is the owner's alone (nobody else can know of it), so iCloud not being ready never holds a send back.
- **Sent, its draft gone, refused for good:** a terminal phase. In `merge` it beats every other version, a newer generation (a concurrent cancel or edit) included, so a sent message is never brought back to be sent again.
- **Sync off:** the owner sends as before; another device's schedule never sends here.
- **The iPhone's own outbox** keeps its messages in files, not in the table. It publishes each send-later message as a record (`Store::publish_scheduled_send`), with its Gmail draft id when the message has one and carries no local files, and asks `scheduled_send_turn` before sending it. A message cancelled or sent elsewhere is then dropped from the phone's outbox.

**Reminders** follow the snooze pattern. Any device may fire a due reminder, which is an idempotent INBOX + UNREAD, but only after a pull that completed after it was due (`reminder_turn`). This way a reminder cancelled, replaced or fired elsewhere isn't fired here. Firing, a reply arriving and the thread being gone are terminal phases. Two devices that are both fresh at the same second may both fire; they send the same label change.

## Storage on the device

Code: `penguin-core/src/store_sync.rs`, schema `SCHEMA_SYNC`.

- **`sync_records(name, kind, key, record, base_tag, server_ms, dirty, changed)`:** one row per record, plus the server change tag it is based on, when the server stored this version, and whether it still has to be sent.
- **`sync_state`:** the device id (random, created with the database), the HLC, `enabled`, the change-feed token and `last_pull_at`.
- **`snoozes` gains `remote_pending` and `wake_undo`.**

Records are written in the **same transaction** as the change to the app's table, always, whether or not sync is on. Turning sync on is just pushing what's pending. Remote records are merged and projected in one transaction too (`LocalStore::apply_remote`).

The transport contract (`penguin_sync::engine`):
- `pull(token)` returns records, deleted names, the next token and `more`.
- `push(batch)` returns `saved(tag, modified)` or `conflict(server record)`.

`sync_once` drives a pull-then-push pass for transports run from Rust, including the in-memory server used in tests.

### Moving records: CKSyncEngine

One Swift file runs the engine on both devices (`CloudSync.swift`): the user's private database, one custom zone `Penguin`, record type `Record` whose one field `payload` (encrypted bytes) is the record's JSON, record name = `record_name`. It never reads a payload. Every decision is in penguin-core's `CloudHost` (`cloud.rs`), reached through C callbacks on the Mac (`CloudSyncBridge.swift` ⇄ `src-tauri/src/cloud.rs`) and UniFFI on the iPhone (`ios/Penguin/Sync/PhoneCloud.swift` ⇄ `penguin-ffi/src/cloud.rs`).

| CKSyncEngine | CloudHost |
|---|---|
| state update | `save_engine_state` (`sync_state.engine_state`) |
| `fetchedRecordZoneChanges` | `apply` (merge, project, settings into the file) / `forget` |
| `willFetchChanges` … `didFetchChanges` | `fetched(started)`: the wake gate opens for wakes before the fetch started; settings carried over the first time |
| `nextRecordZoneChangeBatch` | `outgoing(names)`; a save is conditional on the archived system fields kept as the base tag |
| `sentRecordZoneChanges` | `saved`; `serverRecordChanged` → `conflict` (merge, re-queue if ours wins); `zoneNotFound` / `unknownItem` → `clear_base` and re-queue |
| account sign-out / switch, zone deleted / reset | `reset` (everything goes up again); `purged` (deleted in iCloud settings) turns sync off |
| account status | `report_account`: records move, and wakes wait for fetches, only with an iCloud account |

New local records reach the engine through the store's pending listener (`Store::set_sync_listener`, after the write commits). A due snooze whose wake waits for a fetch asks for one (the Mac's scheduler; the iPhone's SnoozeClock fetches and wakes again). Both fetch when the app comes forward, and CloudKit's silent pushes wake the engine.

The switch (Settings → Sync with iCloud) is on unless turned off (`sync_state.wanted`). The engine runs only in a build with an iCloud container: the Mac checks its entitlements and needs macOS 14 (CKSyncEngine); everywhere else sync says why it's unavailable.

## Migration of the Mac's existing data

- On first open with this build, `store_sync::seed` gives every existing active snooze and every dismissal a record, once (`sync_state.seeded`). They are queued, so the first sync uploads them and the phone gets them.
- Woken and pushed snoozes are only sort keys and are not seeded.
- Settings are carried over after the device's first complete fetch (`CloudHost::fetched`, once, `sync_state.settings_seeded`): every synced key and snippet or signature that differs from the app's defaults and has **no record yet** becomes a *seeded* record (`Record::seeded`, generation zero), which any real edit on any device outranks. A device that joins later therefore takes the settings already synced and only adds what nobody synced.
- On a second device, local and remote copies merge by the normal rules. Nothing is overwritten wholesale, so no data is lost whichever device is set up first.

### Settings rules

- Every local write of the settings file is recorded **before** the file is written (`CloudHost::record_settings_change`); records that win a merge are written into the file normalized like any other write (the Mac sanitizes signature HTML again), without being recorded again. At launch the file is brought in line with the records (`reconcile_settings`).
- A value this build can't read is skipped, not allowed to sink the file (`apply_settings_records`).
- **The iPhone keeps a subset** (smart views without the Mac's pinned searches and counts, snippets without files): its changes are laid over the last synced value (`settings::overlay`), so what it lacks survives.
- **Each device only shows its own accounts**, so a write keeps the entries of accounts it doesn't have (`keep_other_devices_accounts`: account order, hidden from All, signature defaults, profile members).
- What syncs and what stays is the table under "Data model". A device can keep any synced key to itself (`keepHere`).

## Security and what is exposed where

| Where | What |
|---|---|
| CloudKit, readable by Apple without ADP and only by the user with ADP | encrypted record payloads (JSON above: account addresses, provider thread ids, times, settings values, snippet and signature text) |
| CloudKit, visible to Apple even with ADP | record names (hashes), record type, zone, sizes, modification times, the device's iCloud identity |
| The developer's CloudKit dashboard | nothing per user: private databases aren't visible to the developer, only aggregate usage |
| Never leaves the device | tokens, passwords, message content, the index |

Thread ids and addresses are metadata, not content, but they still go only inside encrypted fields. If a user resets their iCloud Keychain, the encrypted zone becomes unreadable (`zoneNotFound`). The shim then deletes and recreates the zone and re-uploads from local records, since every device keeps a full copy.

## How the iPhone uses it

The phone links the same Rust core through UniFFI: penguin-core's `Store` with `store_sync.rs`, plus `penguin-sync`.
- The same `CloudSync.swift` (listed in `ios/project.yml`) on `PhoneCloudHost`, the UniFFI core (`penguin-ffi/src/cloud.rs`).
- Its snooze scheduler is the same `penguin_provider::snooze::run_due`, with the same wake gate; `cloud_wake_needs_fetch` tells SnoozeClock to fetch and wake again.
- It runs from silent pushes (`remote-notification` background mode), coming forward and background app refresh, and wakes snoozes the Mac made when the Mac is asleep, and the other way round.

## Handoff

Separate from the synced records above: Apple's Handoff (NSUserActivity), between the user's own devices signed in to the same iCloud account, for what is on screen right now (Forge PEN-58).
- Reading a conversation on one device offers it on the other; a draft being written does too, once the composer's autosave has saved it to Gmail. The draft's content goes through Gmail, not Handoff.
- Two activity types, declared in each app's `NSUserActivityTypes`: `co.gluska.penguin.conversation` (`{v: 1, account, thread}`) and `co.gluska.penguin.draft` (`{v: 1, account, draftId, thread?}`), title = the subject. Gmail accounts only. An unknown `v`, a malformed field or an account not signed in on the receiving device ends in a short message, never a crash.
- The Mac side: `src-tauri/src/handoff.rs` (contract, validation, which window's activity is advertised, receiving), `Handoff.swift` in the same Swift package as the sync shim, and `src/features/handoff/`; details in `docs/ARCHITECTURE.md` (Handoff with the iPhone).
- Nothing to set up: Handoff needs the same Apple team on both apps (it is) and no entitlement, container or capability.

## Apple setup

Done (2026-10-12):
- CloudKit container `iCloud.co.gluska.penguin`; App ID `co.gluska.penguin` (Mac and iPhone) with iCloud (CloudKit) and Push Notifications (checked through the App Store Connect API).
- The Mac's Developer ID provisioning profile (`MAC_APP_DIRECT`, made through the API with the Developer ID Application certificate) is the repo secret `PENGUIN_MAC_PROVISION_PROFILE`. release.yml embeds it and signs with entitlements read from it (`apps/desktop/scripts/mac-cloud-signing.sh`): application and team identifiers, the container, `icloud-services = CloudKit`, container environment Production, `aps-environment = production`. macos-check builds the app signed that way, launches it and checks sync is available.
- The iPhone's entitlements are in `ios/project.yml` (container `iCloud.$(PRODUCT_BUNDLE_IDENTIFIER)`, `aps-environment`); Xcode Cloud's automatic signing picks them up.
- No Google Cloud Console change and no re-consent: no new OAuth scope.

Still to do, once: **deploy the CloudKit schema to Production.** TestFlight and Developer ID builds talk to Production, where a record type can't be created by a save. In CloudKit Console (icloud.developer.apple.com → CloudKit Database → `iCloud.co.gluska.penguin`):
1. Development environment → Schema → Record Types → New Type `Record`; add field `payload`, type Bytes, Encrypted on. Save. (Or `xcrun cktool import-schema` with `docs/cloudkit-schema.ckdb` and a management token.)
2. "Deploy Schema Changes…" → Deploy to Production.
Until then saves fail and Settings says "iCloud isn't ready for Penguin yet"; fetching works and nothing local is lost.

## Open-source builds

The container id, team id and provisioning profile are build-time values, like the OAuth clients (see `docs/google-setup.md`).
- A self-build without them compiles the shim out, and sync shows as unavailable.
- A self-build with its own Apple team gets its own container.
- No identifier of the maintainer is hard-coded in source.

## Tested

- **`cargo test -p penguin-sync`:**
  - clocks: monotonicity, a slow clock follows what it saw, clamping;
  - merge: newer intent wins, phase ordering, equal-stamp determinism, order independence across all permutations;
  - two devices through the in-memory server:
    - concurrent snooze edits;
    - an offline re-snooze vs a wake;
    - unsnooze vs wake;
    - delete vs concurrent edit;
    - fast-clock clamping;
    - an offline queue;
    - paged pulls;
    - push conflicts merged and retried;
    - collected tombstones;
  - settings diff and apply.
- **`cargo test -p penguin-core --lib sync`** (two real Stores through the in-memory server):
  - a snooze crosses devices and waits for the archive;
  - the wake is committed once and the other device only follows;
  - an offline re-snooze undoes an unpushed local wake, labels included;
  - unsnooze crosses;
  - 404 and Ended don't override a newer snooze;
  - dismissals sync both ways;
  - records wait for missing accounts and survive account removal;
  - seeding pre-existing rows;
  - the gate stays open with sync off;
  - settings records round-trip.
- **`cargo test -p penguin-provider snooze`:** the wake push waits for a fresh pull; a 404 ends the snooze for every device.
- **`cargo test -p penguin-core --lib cloud`** (a Mac and an iPhone, each a real Store and a settings file, through a fake shim doing what CloudSync.swift does): settings both ways with the phone's subset kept, a joining device taking synced settings and carrying over only the rest, concurrent edits conflicting, a snippet's files surviving a phone edit, a snooze crossing and its wake waiting for a fetch, the pending listener, the switch and the account, starting over on a new account or zone, purged data turning sync off, unreadable values skipped, the file reconciled at launch.
- **`cargo test -p penguin-sync`** also: seeded records lose to any edit, overlay, seed changes, keeping other devices' accounts, the wider settings set, `keepHere` (kept keys neither recorded nor overwritten, letting go takes the synced value), the iPhone's signatures with the Mac's extra fields kept; the outbox rule (owner on time, late owner, takeover window, no takeover without a checkable send or a draft, claims sending only once stored, stale claims, terminal beats newer generations, the stored claim beats a concurrent one whatever the clocks, two devices claiming at once through the server in either order, reminders after a fresh pull).
- **`cargo test -p penguin-core --lib store::outbox`** (a Mac and an iPhone, real Stores, the in-memory server): a schedule shows elsewhere and isn't due there before takeover; the owner sends and the other forgets it; an asleep owner taken over once, yielding when it wakes; concurrent claims leave exactly one sender either way round; a never-synced owner just sends; IMAP only by its owner; sync off; cancels both ways; sent beats a concurrent cancel; rescheduling; retries keep the owner and record nothing; the phone's published schedules; records waiting for their account; reminders crossing, firing once after a fresh pull, replacing each other.
- **`cargo test -p penguin-ffi --lib cloud`** and **`-p penguin-desktop --lib settings`:** each app's settings hook and synced writes (signatures sanitized again, Keep mail for never synced).
- **CI:** macos-check compiles the shim with Xcode 26.0.1, 26.3, 26 and 27, and builds, signs (Developer ID, iCloud entitlements, embedded profile) and launches the app; ios-check compiles the phone's side.

## Gaps

- **Snippet files.** A snippet syncs without its files: they stay on the Mac that has them (a phone edit keeps them). Files can be up to 25 MB, and a CloudKit record holds about 1 MB. Two ways to carry them:
  - chunked `snippetFile` records (keyed by the file's hash, under 750 KB each), which keep the one `Record` type and need no schema change;
  - a CKAsset field on `Record`, which needs a schema change and a deploy in CloudKit Console.
  Either way, the iPhone's composer would also need to keep snippet files. It keeps none today.
- **The iPhone's own send-later messages carrying files** are published without a draft id. The Mac shows them and can cancel them, but can't take them over: the files are only on the phone.
- **The race the claim can't close:** an owner that sent a message but went offline before its "sent" record reached iCloud, while another device took it over 15 minutes later. For Gmail the second send finds the draft already sent (a 404, counted as sent). IMAP and SMTP are never taken over, so the race can't happen there.
