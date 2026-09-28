# Implementing a mail provider (IMAP, Microsoft Graph)

The brief for the IMAP and Microsoft agents. Phase 0 (this seam) is done: the
app reaches every mailbox through `penguin-provider`'s traits, Gmail
implements them by wrapping penguin-gmail, and `Account.provider` says which
backend serves an account. Your job is a crate that implements the same
traits, plus a handful of one-line hooks. Background and research:
`docs/providers-and-onboarding.md`. The command contract (`connect_account`,
`Account`) is in `docs/ARCHITECTURE.md`.

**Rules that don't change:** penguin-core stays network-free. Never log
bodies, subjects, addresses, passwords or tokens (ids and codes only).
Fixtures use fictional people and `.example` domains. Keep `types.rs` ⇄
`types.ts` in lockstep. Build and test only through `scripts/box-cargo.sh`.

## 1. Layout

```
crates/penguin-provider   the seam (no Tauri, no network of its own)
  provider.rs             MailProvider, Backend, SyncObserver, SyncHandle/SyncTask,
                          DraftRef, OpenedDraft, MessageMetadata, LabelUpdate, ServerSearch
  error.rs                Error (the one error type every provider returns)
  compose.rs              outgoing MIME (Draft → RFC 822), shared by all providers
  outbox.rs, snooze.rs    send later, reminders, snooze wakes (run through MailProvider)
  window.rs               WindowPolicy, OlderMail, window_start_ms
  ids.rs                  id rules, IMAP id/thread helpers, label-id encoding, system labels
  credentials.rs          Keychain vaults: PasswordCredential, MicrosoftCredential, SecretVault
  fake.rs                 FakeProvider / FakeBackend (feature "fake")
  conformance.rs          the conformance suite (feature "fake")
crates/penguin-gmail      provider.rs: GmailProvider + GmailBackend (reference implementation)
crates/penguin-imap       YOU (IMAP agent): new crate
crates/penguin-graph      YOU (Microsoft agent): new crate
apps/desktop/src-tauri
  providers/registry.rs   backend per AccountProvider + client cache (dispatch)
  providers/mod.rs        install_backends() (register yours), connectable() (flip yours)
  providers/connect.rs    connect_account + connect_imap / connect_microsoft / reconnect stubs
  state.rs                AppState::provider(account_id) → Arc<dyn MailProvider>
```

Your crate depends on `penguin-core` and `penguin-provider` (and
`penguin-render` if you need it), never on `penguin-gmail` or Tauri. Add it to
`[workspace.dependencies]` and to `apps/desktop/src-tauri/Cargo.toml`.

## 2. The interface

### 2.1 `MailProvider` (one account)

`#[penguin_provider::async_trait] impl MailProvider for YourProvider`. One
instance per account, built by your `Backend::client` and cached by the app;
hold your client, credential access and a `Store` clone. All methods are
interactive (the user is waiting): never queue them behind the sync.

| method | contract |
|---|---|
| `account_id()`, `provider()` | the account id; `AccountProvider::Imap` / `::Microsoft` |
| `capabilities()` | defaults to `provider().capabilities()` (the table in `types.rs`); override only to report *less* at runtime |
| `fetch_messages(ids)` | full messages, one `(id, Result<Option<Message>>)` per id **in input order**; `Ok(None)` = gone from the server (not an error) |
| `fetch_pending_bodies(ids)` | download + `upsert_messages` the bodies of headers-only messages; skip ids already in flight or no longer `is_body_pending`; return changed thread ids; error only when nothing could be stored |
| `get_attachment(message_id, meta)` | bytes of `meta.id` (your id). The app caches them |
| `get_message_metadata(id)` | every header, server order, + size; `None` = gone (feeds Message details and the one-click unsubscribe check) |
| `get_message_raw(id)` | RFC 822 bytes; `None` = gone ("Show original") |
| `modify_thread(thread_id, add, remove)` | apply canonical label deltas to **every message of the thread** (look them up in the store). §4 says what each label means for you. Unknown thread → `NotFound` |
| `trash_thread(t)` / `untrash_thread(t)` | to Trash; out of Trash **and into the inbox** |
| `update_label`, `delete_label` | only with `capabilities.label_edit` (defaults return `Unsupported`) |
| `ensure_label` | find or create a user label / folder / category by name (Reply Later). Gmail labels.create, IMAP CREATE + SUBSCRIBE, Graph a `c:` category with no call. Every provider implements it |
| `send_raw(raw, thread_id)` | send bytes built by `compose` (Bcc header present: strip it before SMTP DATA / let Graph do it); return `SentRef` ids that your sync will store (IMAP: APPEND to Sent unless the server saves it itself) |
| `save_draft(draft, from, draft_id, attachments)` | create/update a server draft **and mirror it locally** (a `Message` labeled `DRAFT` via `upsert_messages`, delete the previous local version, `Store::set_draft(draft_id, message_id)`). A draft gone on the server is recreated. Return `DraftRef.attachments` as refs to the stored copy in the order they were sent (`penguin_provider::inline::saved_refs`: inline images matched by Content-ID, files in order). Inline images (an attachment with `content_id` the HTML shows, `inline::plan`) must survive: MIME providers get them as `multipart/related` from `compose`; Graph uploads them with `isInline` + `contentId`. See `penguin-gmail/src/drafts.rs::save` |
| `send_draft(...)` | send with the final content, remove the draft (server + local copy + mapping) |
| `send_saved_draft(draft_id)` | send the draft exactly as stored (send later). **A second call must be `NotFound`** (that's what makes retries unable to double-send) |
| `delete_draft(draft_id)` | server (missing = fine) and local; return its thread if it was stored |
| `open_draft(draft_id?, message_id?)` | reopen for editing; `body_html` and attachments from `penguin_provider::inline::reopen` (the editor allowlist keeping only the `cid:` images of the draft's own inline parts, which come back as refs with `content_id`; conformance checks the round trip) |
| `server_search(parsed, cap)` | optional (`server_search`): translate `penguin_core::query::ParsedQuery`, store ≤ `cap` unknown matches with `insert_header_messages`, return hits (one per thread, newest first) |
| `window_estimate(months, now)` | optional (`window_estimate`): messages since `window_start_ms(now, months)` (0 = mailbox) |
| `profile_photo()` | optional (`profile_photo`): raster bytes or `None` (Graph `/me/photo/$value`) |

Everything that talks about "drafts" follows Gmail's model: a **draft id is
stable** across saves, the draft's **message id may change** on every save;
the `drafts(account_id, draft_id, message_id)` table maps them. Graph: draft
id = message id (immutable id). IMAP: mint `d:<random>` as the draft id; each
save APPENDs a new message (new message id) and expunges the old one.

### 2.2 `Backend` (one provider kind)

| method | contract |
|---|---|
| `provider()` | your kind |
| `client(account)` | `Arc<dyn MailProvider>` for the account (cheap; the app caches it) |
| `has_credentials(account)` | Keychain has what the account needs (may block; called on the blocking pool; one prompt per process at most) |
| `sign_out(account)` | delete credentials (Graph: nothing to revoke server-side for public clients; just forget). Best effort |
| `start_sync(account)` | spawn (or return the running) per-account task; `SyncHandle::new(Arc<impl SyncTask>)` |
| `retry_sync(account)` | clear the error status (emit it), restart a dead task or wake it from backoff |
| `sync_status(account_id)` | last `SyncStatus` of the task (`None` if it never ran) |
| `set_window_policy(policy)` | live Settings → Sync values for every running account |

Construct it with the app's `Store` and `state.sync_observer()` and register
it in `providers::install_backends` (one line):

```rust
state.register_backend(Arc::new(penguin_imap::ImapBackend::new(state.store.clone(), state.sync_observer())));
```

### 2.3 The sync task

Model it on `penguin-gmail/src/sync.rs` (`AccountSync`, `Shared`,
`SyncEngine`): a tokio task per account, `catch_unwind` so a panic never
takes down other accounts, a 1 s status heartbeat while backfilling, error
backoff 5 s → 5 min, a poke wakes it.

- Report through the `SyncObserver`: `status(SyncStatus)` on every phase
  change (`Backfilling` with `stage` Window/Older + `rate_per_min`/`eta_secs`,
  `Incremental`, `Idle`, `Error` with a short message, `NeedsReauth`);
  `mail_changed(account, thread_ids)` after every committed change;
  `messages_added(account, ids)` for **new mail from incremental sync only**
  (not backfill), after commit and **before** saving the cursor that covers
  it (rules replay after a crash); `labels_added` for labels added to stored
  messages (optional; rules' "label added" trigger).
- `NeedsReauth` / `Keychain` errors stop the task (phase `NeedsReauth`);
  never retry them on a timer.
- Follow the window policy: in-window mail in full (`upsert_messages`),
  older mail per `OlderMail` (headers-only with `insert_header_messages`,
  full, or none). Newest first; commit in chunks (~100); new mail during a
  long backfill must not wait for it.
- Park messages that keep failing on their own in
  `cursor.failed_message_ids` and retry them in bounded batches, as Gmail does.
- Re-read the label/folder list periodically and `replace_labels`.

## 3. Identity

The store never parses ids; it keys everything by `(account_id, id)`.
Existing Gmail rows keep their Gmail ids. Rules (`ids::check_id`): non-empty,
≤ 512 bytes, printable ASCII **without spaces** (label sets are stored
space-separated), not starting with `~` (reserved).

| | message id | thread id | attachment id |
|---|---|---|---|
| Gmail | Gmail id | Gmail thread id | Gmail attachment id (re-stored messages keep old ids) |
| Microsoft | immutable id (send `Prefer: IdType="ImmutableId"` on **every** request, or ids change on move) | `conversationId` | attachment id |
| IMAP | `e:<EMAILID>` when the server has OBJECTID (RFC 8474: `ids::imap_message_id_from_emailid`), else `ids::imap_fallback_message_id(Message-ID, Date, RFC822.SIZE)` (`h:` + hash; the same message in two folders gets one id) | `ids::imap_thread_id(root Message-ID)` (`t:` + hash); JWZ over Message-ID / In-Reply-To / References, subject merging only within a time window | MIME part path (`1.2`) |

- IMAP UIDs are locations, not identities: keep `(folder, uidvalidity, uid)
  → message id` in **your own table** (§7). A move changes the UID, not the
  message id. UIDVALIDITY change → rescan that folder, re-map, never
  duplicate.
- When threading merges two conversations, move the messages with
  `Store::set_message_thread(account, ids, thread_id)` (recomputes both
  threads, removes the emptied one, returns the changed thread ids for
  `mail_changed`). Don't mix JWZ with Microsoft's `conversationId`.
- `Message.message_id_header`, `in_reply_to`, `references`: **without angle
  brackets** (`ids::normalize_message_id` for comparisons).
- `Message.date`: unix **ms** (IMAP INTERNALDATE; Graph `receivedDateTime`).

## 4. Labels and folders

The store, views, search, rules and the UI speak Gmail's label model. Map onto
it; don't extend it.

**System labels** (canonical ids, `ids::system`): `INBOX SENT DRAFT TRASH
SPAM STARRED UNREAD IMPORTANT`. Non-Gmail providers emit no others (no
`CATEGORY_*`, no raw IMAP flags). Views: Inbox = `INBOX`; Done/archive = none
of INBOX/TRASH/SPAM; Sent, Drafts, Trash, Spam, Starred by label; user labels
by id.

**User labels**: `Label { id, name, kind: "user" }` with ids from
`ids::label_id_for_name(prefix, name)` (`f:` IMAP folders — the full path,
`c:` Microsoft categories, `f:<folder id>` Graph folders); `name` is the
display path with `/` for nesting. `kind: "system"` only for the canonical ids.

| provider state | labels |
|---|---|
| IMAP `\Inbox` (INBOX) | `INBOX` |
| `\Sent`, `\Drafts`, `\Trash`, `\Junk` | `SENT`, `DRAFT`, `TRASH`, `SPAM` |
| `\Archive`, `\All` (and "Archive" by name) | nothing (archived = no INBOX) |
| other folder | `f:<path>` |
| `\Seen` absent / `\Flagged` | `UNREAD` / `STARRED` |
| `$Important` (if you choose) | `IMPORTANT` |
| Graph well-known folders inbox/sentitems/drafts/deleteditems/junkemail/archive | as the IMAP rows |
| Graph `isRead=false` / `flag.flagStatus=flagged` | `UNREAD` / `STARRED` |
| Graph categories | `c:<name>` (user labels, several per message) |

A message present in several IMAP folders carries the union.

**Label deltas → operations** (`modify_thread` is called with canonical ids;
these are what the app sends: archive = remove INBOX, move to inbox = add
INBOX, read/unread = ∓UNREAD, star/unstar = ±STARRED, label ±id, trash/untrash
via their own calls; snooze = archive, wake = +INBOX +UNREAD; reminders =
+INBOX +UNREAD):

| delta | IMAP | Graph |
|---|---|---|
| −UNREAD / +UNREAD | STORE ±FLAGS `\Seen` | PATCH isRead |
| ±STARRED | STORE ±FLAGS `\Flagged` | PATCH flag |
| −INBOX (archive) | MOVE INBOX → `\Archive` (else `\All`; else create "Archive") | move to archive |
| +INBOX | MOVE to INBOX (from wherever the thread's messages are) | move to inbox |
| +`f:X` (folders capability) | MOVE to X (IMAP has no multi-label; UI shows it as "Move to") | move to folder |
| −`f:X` | MOVE X → Archive | move to archive |
| ±`c:X` | — | PATCH categories |
| +SPAM / −SPAM | MOVE to/from `\Junk` | move to/from junkemail |

Trash: MOVE to `\Trash` (Graph: deleteditems). Untrash: MOVE to INBOX. Keep
the local optimistic state right: the app already applied the delta locally;
after success it pokes your sync, which must converge to the server's truth.

## 5. Cursors (`SyncCursor`)

`Store::get_sync_cursor` / `set_sync_cursor`, saved after every page:

- `provider_state: String` — **yours, opaque to everyone else.** JSON by
  convention, with a version so you can migrate it: `""` = nothing yet.
  - IMAP: `{"v":1,"folders":{"<path>":{"uidvalidity":…,"uidnext":…,"highestmodseq":…,"backfillBelowUid":…}}}`
  - Graph: `{"v":1,"folders":{"<folder id>":{"deltaLink":"…","backfillNext":"…"}}}` (delta is per folder; 410 Gone → drop that link and resync the folder)
  - Gmail (reference): `{"historyId":…,"backfillPageToken":"…"}` (`GmailCursor`).
  Keep it small (well under 64 KB); per-folder maps are fine, per-message
  data belongs in your own table.
- Generic fields the app reads (keep them honest): `backfill_done` (the
  window fill finished), `failed_message_ids` (parked ids; diagnostics shows
  the count), `window: WindowCursor` (`full_since_ms` = every message dated
  ≥ it has its body, `None` until the first fill finishes, `0` = all mail;
  `older_done` / `older_before_ms` / `older_mode` for the older-mail pass).
  Sync coverage, "free up space" and diagnostics are computed from these.
- Record the incremental anchor **before** backfilling (Gmail records the
  history id first), so nothing that arrives during a long backfill is lost.

## 6. Errors

Return `penguin_provider::Error`. The app maps it (`src-tauri/src/error.rs`):

| situation | return | UI / sync |
|---|---|---|
| bad or revoked credentials, `invalid_grant`, IMAP `AUTHENTICATIONFAILED` during sync | `NeedsReauth(msg)` | needsReauth; sync stops |
| Keychain refused | `Keychain(msg)` | sync stops (no retry storm) |
| offline, DNS, TLS, timeout, connection reset | `Network(msg)` | network; backoff |
| throttled (Graph 429 + `Retry-After`, IMAP `[THROTTLED]`/BYE) | `RateLimited` (after honoring Retry-After inside the client) | network; backoff |
| HTTP error from Graph | `Http { status, body }` (body = Graph's error JSON, never user content) | 404 → notFound, 429/5xx → network |
| object gone (IMAP UID missing, draft gone) | `NotFound(msg)` | notFound; outbox/snooze treat it as "gone" |
| the server refused what the user asked (bad password at connect, name taken) | `InvalidInput(user-facing text)` | invalidInput, message shown |
| capability off | `Unsupported(...)` (`Error::unsupported(provider, what)`) | other |
| consent (`AADSTS65001` …) at sign-in | `OAuth("AADSTS65001: …")` → the connect step returns it as `CmdError::other` with the code in the message; the UI shows the admin-consent screen (`microsoftErrorText`) | |

`Error::is_not_found()` covers `NotFound` and HTTP 404; outbox and snooze use it.

## 7. Store access

Typed API first: `upsert_messages`, `insert_header_messages`,
`delete_messages`, `modify_message_labels`, `known_message_ids`,
`get_message`, `get_thread`, `replace_labels`, `upsert_label`,
`is_body_pending`, `count_messages`, the drafts mapping
(`set_draft`/`draft_for_message`/`message_for_draft`/`remove_draft`/
`replace_drafts`), `set_message_thread`, `get/set_sync_cursor`. All blocking:
call them through `spawn_blocking`.

Your own tables (IMAP locations, Message-ID index for JWZ, Graph folder
map): **never edit `MIGRATIONS`**. Call once at startup (e.g. in your
backend's constructor):

```rust
store.migrate_provider_schema("imap", &[IMAP_V1, IMAP_V2], &["imap_locations", "imap_msgids"])?;
```

— an append-only list per provider, tables named `<name>_…`, each listed
table has an `account_id` column and is emptied by `Store::remove_account`.
Read/write them with `store.provider_read(|conn| …)` /
`store.provider_write(|tx| …)` (`penguin_core::rusqlite` is re-exported so
you use the same version). Delete your rows for messages you delete.

## 8. Credentials (Keychain)

`penguin_provider::credentials`: one Keychain item per provider (service
`co.gluska.penguin.imap` / `co.gluska.penguin.microsoft`, account
`credentials.v1`), JSON `{"version":1,"accounts":{<account id>: …}}`.

```rust
static VAULT: OnceLock<SecretVault<PasswordCredential>> = OnceLock::new();
let vault = VAULT.get_or_init(|| SecretVault::keychain(IMAP_SERVICE));
vault.save(&account.id, &PasswordCredential { password, smtp_password: None, saved_at })?;
```

`PasswordCredential { password, smtp_password?, saved_at }`,
`MicrosoftCredential { refresh_token, client_id, scopes, tenant_id?, object_id?, obtained_at }`.
One vault per process (a `OnceLock`), so the Keychain is read once. Access
tokens stay in memory; refresh single-flight per account (see Gmail's
`AuthManager::access_token`). Server settings and the client id are **not**
secret: they live in `Account.provider_config`.

## 9. Connecting accounts

`providers/connect.rs` already does the shared part: validate the request
(address, required fields, `plain` only to localhost), refuse an address
already in Penguin, build the `Account` (`provider` from the auth method,
`provider_config` from the request, a color), and after your step `finish()`
stores it, starts its sync and returns it. Replace **only your function's
body**:

- IMAP → `connect_imap`: log in to `request.imap`, check `request.smtp`
  (EHLO + AUTH, no send), bad credentials → `InvalidInput` with the server's
  words, unreachable → `Network`; STARTTLS on Proton Bridge's self-signed
  certificate is allowed only for `127.0.0.1`/`localhost`. Save the password,
  return the account.
- Microsoft → `connect_microsoft`: PKCE browser sign-in on `/common` with
  `request.client_id`, loopback `http://localhost:<port>` (reuse the ideas in
  `penguin-gmail/src/auth/loopback.rs` + `pkce.rs`; copy, don't depend),
  scopes `openid profile email offline_access User.Read Mail.ReadWrite
  Mail.Send MailboxSettings.Read`, `login_hint` = the address, cancellable:
  `let cancelled = state.begin_sign_in();` and `select!` on it → `CmdError::cancelled()`.
  Refuse a different account than the one typed.
- `reconnect` (same file): your half of `reconnect_account`.
- Then flip `providers::connectable` for your auth method(s). The UI needs
  no change (it already calls `connect_account` and reads `available`).

## 10. Capabilities

`AccountProvider::capabilities()` in `penguin-core/src/types.rs` is the one
table (mirrored for the mock UI in `src/lib/mock/accounts.ts`). Current values:

| | Gmail | IMAP | Microsoft |
|---|---|---|---|
| labels (multi-label) | ✓ | – | ✓ (categories) |
| folders (move semantics) | – | ✓ | ✓ |
| labelEdit / labelColors | ✓ / ✓ | – | – |
| inboxCategories (tabs, IMPORTANT) | ✓ | – | – |
| serverSearch, windowEstimate | ✓ | ✓ | ✓ |
| drafts, sendLater, snooze | ✓ | ✓ | ✓ |
| calendar, contactPhotos | ✓ | – | – |
| profilePhoto | ✓ | – | ✓ |
| messageHeaders, rawSource | ✓ | ✓ | ✓ |

Change a value only when your implementation really differs (update both
the Rust table and the mock mirror, and say so in your report). The app
already gates on: calendar (Settings → Calendar, sync, connect), contact
photos, profile photo, server search. The UI receives `Account.capabilities`
but doesn't hide anything yet for folders/labels/label editing: when your
provider lands, gate "Label as…"/label editing in the UI on `labels` /
`labelEdit` and show "Move to…" for `folders`.

## 11. Tests (what "done" means)

1. **Unit tests** in your crate with fictional `.example` fixtures: MIME →
   `Message` conversion, folder/flag → label mapping, delta → command mapping,
   id helpers, cursor (de)serialization, error mapping. Run
   `penguin_provider::conformance::check_message` on every converted message
   and `check_labels` on your label list: both must return no problems.
2. **Conformance**: `penguin_provider::conformance::run(&provider, &store, &Case {…})`
   must return an empty list against:
   - IMAP: a disposable server in a container (Dovecot / GreenMail /
     Stalwart), seeded by APPEND with one inbox message with an attachment;
     `send_to` pointing at a second local mailbox. Mark the test `#[ignore]`
     unless the server is reachable (env var), and document the command.
   - Graph: recorded HTTP fixtures (fictional data) behind a fake transport.
   `penguin-provider/src/conformance.rs` shows it passing on the fake.
3. **Sync engine tests** against a fake transport (like
   `penguin-gmail/src/sync_tests.rs`): backfill resumes after restart,
   incremental picks up adds/deletes/flag changes, UIDVALIDITY reset (IMAP) /
   delta 410 (Graph) resyncs without duplicates or data loss, a message moved
   between folders keeps its id, `messages_added` fires for new mail only.
4. `scripts/box-cargo.sh test -p penguin-core -p penguin-gmail -p
   penguin-render -p penguin-provider -p penguin-desktop -p <your crate>` all
   green, `scripts/box-cargo.sh fmt`, UI `npx tsc --noEmit -p . && npm test`.
5. Real accounts (the maintainer, not agents): outlook.com, an M365 dev
   tenant, iCloud, Yahoo, Fastmail.

## 12. Checklist

- [ ] New crate `crates/penguin-<imap|graph>`; workspace dependency; desktop dependency.
- [ ] `YourProvider: MailProvider` — every required method, optional ones per capability.
- [ ] `YourBackend: Backend` with a per-account sync task reporting through `SyncObserver`.
- [ ] Ids per §3 (`check_id` passes); IMAP location table via `migrate_provider_schema`.
- [ ] Labels per §4; `modify_thread` / trash / untrash translate deltas; archive = no INBOX.
- [ ] Drafts per §2.1 (stable draft id, local mirror, `send_saved_draft` twice = NotFound).
- [ ] Sending: `send_raw` + Sent copy handling (no duplicates when the server saves it).
- [ ] Cursor per §5, saved after every page; anchor recorded before backfill.
- [ ] Window policy honored; coverage fields (`backfill_done`, `window`) maintained.
- [ ] Errors per §6; auth failures stop sync with NeedsReauth.
- [ ] Credentials per §8; never logged.
- [ ] `connect_imap` / `connect_microsoft` and `reconnect` implemented; `connectable` flipped.
- [ ] Backend registered in `providers::install_backends` (one line).
- [ ] `Message.sender_authenticated`: false unless your server's own
      Authentication-Results (its authserv-id) says DMARC pass or aligned
      DKIM pass. It gates auto-loaded images and one-click unsubscribe, so
      when unsure leave it false.
- [ ] `list_unsubscribe` / `list_unsubscribe_post` captured at ingest (`Some(bool)`).
- [ ] `snippet`: the first ~200 characters of the text body, whitespace collapsed.
- [ ] Tests per §11; conformance passes.
- [ ] Docs: your section in `docs/ARCHITECTURE.md` (the command table rows you
      changed, a Providers paragraph), and this file if the contract moved.

## 13. Shared files: touch only these lines

Both agents work in parallel; conflicts should be one-line merges:

- `Cargo.toml` `[workspace.dependencies]`, `apps/desktop/src-tauri/Cargo.toml`: add your crate.
- `providers/mod.rs`: your line in `install_backends`, your arm in `connectable`.
- `providers/connect.rs`: the body of your `connect_*` function and your arm in `reconnect`.
- `types.rs` / `types.ts` / `mock/accounts.ts`: only if a capability value changes.
- Never: `MIGRATIONS` (use provider schemas), `penguin-gmail`, the store's typed API
  (ask for an addition in your report if you truly need one).

## 14. IMAP (landed): what it does and how it's tested

`crates/penguin-imap`. Module map: `proto` (response reader + tokenizer:
every FETCH item, code and extension response comes out as atoms, strings
and lists, so an unknown extension can't break parsing; modified UTF-7),
`net` (TCP + rustls with the OS verifier; any certificate only for
loopback hosts), `session` (IMAP commands), `smtp` (submission client),
`folders` (special-use → labels), `mime` (RFC 822 / BODYSTRUCTURE →
`Message`, IMAP part paths as attachment ids), `threading` (JWZ-lite),
`db` (provider tables), `cursor`, `search`, `account` (per-account context
shared by provider and sync), `provider`, `sync`, `backend`, `connect`.

Choices worth knowing:
- Own IMAP client instead of async-imap/imap-codec: `imap-proto` (under
  async-imap) fails whole responses on items it doesn't model (EMAILID,
  X-GM-*, VANISHED), and imap-codec has no Gmail extensions. The client is
  small, generic in what it parses, and tested byte-level. SMTP likewise
  (shares the TLS rules and error mapping; no lettre).
- Gmail quick setup: X-GM-EXT-1 switches to All Mail + Trash + Spam, ids
  `gm:<hex X-GM-MSGID>`, threads `gt:<hex X-GM-THRID>`, labels from
  X-GM-LABELS (user labels as `f:<name>`, so the folder UI works: "Move to"
  = add the label and archive), X-GM-RAW for server search. Drafts and sent
  copies are stored from their All Mail copy; replaced drafts go through
  Trash and are expunged there (expunging from Drafts would archive them).
  Workspace on Gmail's servers is refused before connecting.
- Capabilities are unchanged from the table in §10 (folders, no labels).

Tests (`scripts/box-cargo.sh test -p penguin-imap`):
- Unit: parser, sequence sets, modified UTF-7, folder roles and name
  fallbacks, Gmail label mapping, MIME conversion (every message through
  `check_message`, labels through `check_labels`), BODYSTRUCTURE paths =
  parser paths, transfer encodings, id rules, cursor shape, delta → flag /
  move plans, JWZ threading and merges, location table, SMTP envelope /
  Bcc stripping / dot-stuffing, error classification, connect hints.
- End to end against the in-process scripted server (`testserver.rs`, real
  TCP, capabilities per test): `conformance::run` passes on a full-featured
  server (MOVE, UIDPLUS, CONDSTORE, QRESYNC, OBJECTID), a minimal one (none
  of those, LOGIN only) and Gmail mode; backfill resume after restart,
  incremental adds / flag changes / moves (same id, not "new") / deletions,
  UIDVALIDITY reset without duplicates, headers-only older mail and lazy
  bodies, Sent copies filed exactly once (server saves vs. APPEND), IDLE
  push, thread merges, NeedsReauth stopping the task, no non-PEEK fetch.
- Real servers (ignored test `real_server_conformance`):

```sh
# GreenMail (IMAP + SMTP that delivers locally; no SPECIAL-USE/CONDSTORE)
docker run -d --rm --name penguin-imap-greenmail -p 127.0.0.1:3143:3143 -p 127.0.0.1:3025:3025 \
  -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled=false -Dgreenmail.users=sam:app-pass-1234@mail.example,bea:bea-pass@lark.example -Dgreenmail.users.login=email' \
  greenmail/standalone:2.1.3
PENGUIN_IMAP_TEST_HOST=127.0.0.1 PENGUIN_IMAP_TEST_PORT=3143 PENGUIN_IMAP_TEST_SMTP_PORT=3025 \
PENGUIN_IMAP_TEST_USER=sam@mail.example PENGUIN_IMAP_TEST_PASS=app-pass-1234 \
PENGUIN_IMAP_TEST_SEND_TO=bea@lark.example \
  scripts/box-cargo.sh test -p penguin-imap real_server_conformance -- --ignored --nocapture

# Dovecot 2.3 (STARTTLS with its self-signed cert on loopback, CONDSTORE/QRESYNC,
# special-use folders from a mounted dovecot.conf; no SMTP: leave the SMTP port and send_to unset)
docker run -d --rm --name penguin-imap-dovecot -p 127.0.0.1:3144:143 \
  -v "$PWD/dovecot.conf:/etc/dovecot/dovecot.conf:ro" dovecot/dovecot:2.3.21
PENGUIN_IMAP_TEST_HOST=127.0.0.1 PENGUIN_IMAP_TEST_PORT=3144 \
PENGUIN_IMAP_TEST_SECURITY=starttls PENGUIN_IMAP_TEST_USER=sam@mail.example PENGUIN_IMAP_TEST_PASS=pass \
  scripts/box-cargo.sh test -p penguin-imap real_server_conformance -- --ignored --nocapture
```

The Dovecot config used: the image's defaults (static password `pass`,
sdbox) with `protocols = imap` and a `namespace inbox` declaring Drafts,
Sent, Trash, Junk, Archive (special_use, `auto = subscribe`) and Receipts.

Known gaps: reconnect can't take a new password yet (remove and re-add);
`sender_authenticated` is only trusted for Gmail (`mx.google.com`); the
sync progress total counts every folder's messages, not the window.
