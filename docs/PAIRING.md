# Pair iPhone

Move the accounts signed in on the Mac (Gmail and Google Workspace, IMAP
accounts such as iCloud, Fastmail and Yahoo, Outlook.com and Microsoft 365)
to Penguin for iPhone in one step, with no server: the Mac shows a QR code,
the phone scans it, the two talk directly over the local network, and the
phone stores the accounts' refresh tokens and app passwords in its Keychain
and starts syncing.

Code: `crates/penguin-pair` (the protocol, shared), `crates/penguin-gmail/src/auth/transfer.rs`
(Gmail token export and import), `penguin-provider` `credentials.rs` (`SecretVault::export`,
`parse_export`: the IMAP and Microsoft vaults' entries), the Mac's `apps/desktop/src-tauri/src/pair_iphone.rs`
and `src/features/settings/PairIphone.tsx`, the phone's `crates/penguin-ffi/src/pair.rs`
and `ios/Penguin/UI/Pair/`.

## What the user does

1. Mac: Settings → Accounts → **Pair iPhone**. A QR code appears, with a
   two-minute countdown and a summary of what can move.
2. iPhone: the Welcome screen's **Pair with Mac**, or Mailboxes → **Pair with
   Mac**. The camera opens; point it at the code.
3. The phone connects and shows "Confirm on your Mac" with a six-digit code.
   The Mac shows the phone's name and model, the same code, and exactly what
   it will send. Nothing is sent until **Send** is clicked.
4. The phone stores the tokens and passwords, adds the accounts with the
   Mac's names, colors, order and (IMAP, Microsoft) server settings, and
   starts syncing. Accounts it can't take a credential for are listed with a
   one-tap **Sign in** (Google's or Microsoft's sheet with the address filled
   in, or the IMAP password screen). Both screens say how it went.

Pairing again later sends only accounts the phone doesn't have yet (PEN-57):
the phone tells the Mac which accounts it already holds a working token for
(one it can refresh). An account the phone lists as needing sign-in is not
among them, so pairing again gives it the Mac's token and it starts syncing.

## Which tokens can move (PEN-28)

A Google refresh token refreshes only with the OAuth client that minted it.
Each token in the v3 Keychain vault records its client (`penguin-gmail`
`auth/keychain.rs`). The phone has its build's **iOS** client
(`PENGUIN_GOOGLE_IOS_CLIENT_ID`, no secret), and since protocol version 2 the
Mac also sends its **Desktop app** client (the id and secret it loaded from
`google-oauth-client.json` or `PENGUIN_GOOGLE_CLIENT_JSON`) whenever it sends
browser sign-ins. Per account:

| Token on the Mac | On the phone |
|---|---|
| Desktop client (browser sign-in), the Mac's current one | **Moves**, with the Mac's desktop client. The phone refreshes it with that client. |
| iOS client, same id as the phone's build | **Moves.** The phone refreshes it like its own. |
| iOS client, a different id | "Sign in on the iPhone" (the clients differ) |
| Desktop client the Mac no longer has (its client file changed) | "Sign in on the iPhone" (it can't refresh on the Mac either) |
| No token (signed out, Keychain locked) | "Sign in on the iPhone" |

IMAP and Microsoft accounts (protocol version 3):

| On the Mac | On the phone |
|---|---|
| IMAP account with its password in the Keychain | **Moves**: the password (app password) and the account's servers |
| Microsoft account with a refresh token | **Moves**, with the Application (client) ID that minted it. Microsoft refreshes a token only with its own client, and a public client needs no secret and no redirect to refresh, so the phone refreshes it with that id whatever Microsoft client its own build has (or none) |
| No password or token on the Mac (removed, Keychain locked) | "Sign in on the iPhone" |

The phone stores them in its own Keychain items, the same vaults the Mac
uses (`co.gluska.penguin.imap` and `co.gluska.penguin.microsoft`, account
`credentials.v1`, `{"version":1,"accounts":{id: credential}}`), and the
servers in the account's `providerConfig`.

**The desktop client on the phone.** Google documents an installed app's
client secret as not confidential (it ships inside every copy of a desktop
app), and this is the user's own client (bring your own credentials), so the
phone needs no build-time configuration for it. It travels only inside the
encrypted transfer. The phone keeps it in its Keychain, in its own item
(service `co.gluska.penguin.google`, account `desktop-clients.v1`,
`{"version":1,"clients":{id: secret}}`, `penguin-gmail` `auth/paired_clients.rs`),
never in a file, and never logs it. It stores the client before the tokens that
need it, and uses it only to refresh tokens whose recorded client is that id:
it never signs in with it (sign-in on the phone is always the iOS sheet).
Tokens from a desktop client the phone doesn't hold read as needing sign-in.
The Mac stamps its desktop client id on tokens stored before ids were recorded,
so the phone can match them.

For iOS-client tokens, both sides take the id from their own configuration
(the Mac from its settings, the phone from its build); the phone sends its id
in the encrypted hello and the Mac compares. When they differ the Mac says so
before Send ("This iPhone signs in with a different Google client…") and sends
those accounts as "sign in" (browser sign-ins still move). The phone checks
again on import and refuses any token it couldn't refresh.

## Versions

The QR text, the hello's first byte and the key-derivation labels carry the
protocol version (`PROTOCOL_VERSION`, now 3). Each side refuses any other
version with an error that says what to do, never a partial pairing:

- An older phone (version 1 or 2) scanning a version 3 code: "This pairing
  code is from a newer version of Penguin. Update Penguin on both devices."
- A version 3 phone scanning a version 1 or 2 code, or a version 3 Mac
  getting a version 1 or 2 hello: "The other device has an older version of
  Penguin. Update Penguin on both your Mac and your iPhone…"

Version 2 added `desktopClient` to the transfer; a transfer without it still
reads (it means no browser sign-ins were sent). Version 3 added
`imapVault`, `microsoftVault` and each account's `provider` and
`providerConfig`; without them the transfer reads as Gmail only. The
version is bumped (rather than the fields just added) so an older phone
never takes a version 3 transfer and silently drops its IMAP and Microsoft
accounts: it refuses the code and says to update.

## Transport

- **Mac:** a TCP listener on every interface (`[::]`, dual stack, or `0.0.0.0`),
  port chosen by the system, opened when the QR code appears and closed when a
  phone pairs, the code expires (2 minutes), the window closes or Cancel.
- **QR code:** `penguin-pair:3?k=<Mac public key>&s=<secret>&p=<port>&a=<hosts>`.
  Hosts are the Mac's IPv4 addresses (private ranges first) and its Bonjour
  name (`Name.local`). No Bonjour browsing: the code already says where to go.
- **iPhone:** Network.framework. One `NWConnection` per host, started together,
  first ready wins, cellular excluded. Frames are a 4-byte big-endian length
  and the body (at most 256 KB).
- **Can't connect:** after 10 seconds the phone says "Couldn't reach your Mac.
  Make sure both are on the same Wi-Fi network…", and if iOS reports that Local
  Network access is off, it says that instead (Settings → Privacy & Security →
  Local Network → Penguin). Guest networks and client isolation block devices
  from reaching each other; the message names that too. The Mac's code stays
  valid for its two minutes, so the user can fix it and scan again.

## Protocol

Both sides are sans-IO state machines in `penguin-pair` (`Offer`/`MacSession`,
`PhoneSession`), tested together in-process and over TCP.

```
Mac                                             iPhone
 Offer: X25519 key m, secret s (32 bytes)
 QR = (M = m·G, s, port, hosts)  ── camera ──▶  parse QR
                                                 fresh X25519 key p, P = p·G
          ◀── hello: 3 ‖ P ‖ AEAD_kp(PhoneHello) ──
 derive keys, open PhoneHello
 (offer spent: listener closed, m wiped)
          ── AEAD_km(Waiting{mac name}) ──────▶  first proof that this is the Mac
 show phone, code; wait for Send
          ── AEAD_km(Transfer) ───────────────▶  import, add accounts, sync
          ◀── AEAD_kp(Done{imported, failed}) ──
```

Key schedule (`crypto.rs`):

```
transcript = SHA-256("penguin-pair/3" ‖ M ‖ P)
prk        = HKDF-Extract(salt = transcript, ikm = X25519(m, P) ‖ s)
kp         = HKDF-Expand(prk, "penguin-pair/3 phone->mac", 32)
km         = HKDF-Expand(prk, "penguin-pair/3 mac->phone", 32)
code       = HKDF-Expand(prk, "penguin-pair/3 code", 4) mod 10⁶
```

AEAD is ChaCha20-Poly1305 with the transcript as associated data and a nonce
of four zero bytes and a per-direction message counter (from 0, never sent).
A receiver opens only the next message it expects. A low-order public key on
either side is refused.

**PhoneHello**: device name, model, iOS version, app version, the phone's
Google client id, and the addresses it already has working credentials for
(any provider).
**Transfer**: the Mac's name, its sign-in-sheet client id, `vault` (the v3
vault JSON, `{"version":3,"accounts":{email: credential}}`, holding only the
Gmail tokens with status `token`), `desktopClient` (`{clientId, clientSecret}`, only
when `vault` holds browser sign-ins), `imapVault` and `microsoftVault` (the
IMAP and Microsoft vault JSON, `{"version":1,"accounts":{id: credential}}`,
only the accounts with status `token`, absent when there are none), and every
account in the Mac's order: address, display name, nickname, color, position,
`provider` (`gmail`, `imap`, `microsoft`), `providerConfig` (IMAP and
Microsoft: the Mac's `Account.providerConfig`, servers and user names or the
client and tenant ids, never a secret) and a status (`token`,
`alreadyOnPhone`, or `signIn` with a reason). **Done**: the addresses stored
and the ones refused, with why.

## Threat model

Assets: Google refresh tokens (full Gmail access until revoked), IMAP app
passwords (full mailbox access, IMAP and SMTP, until the user deletes the app
password at the provider; an account password where the server takes one,
which may unlock more than mail), Microsoft refresh tokens (mail read, write
and send until revoked or expired), the account list with each account's
servers and user names, and the Mac's Desktop app client secret. The secret is not confidential
by Google's design for installed apps and is no use without a refresh token,
but it crosses the same channel as the tokens and gets the same protections
(P1, P2, P6, P10). Out of scope: a compromised Mac or phone (it already holds the
tokens), and someone who has the unlocked phone in hand.

| # | Threat | Defense |
|---|---|---|
| P1 | Someone on the same network reads the transfer | Everything after the hello is encrypted under keys that need the Mac's private key and the QR secret. |
| P2 | Man in the middle on the network (ARP or DNS spoofing, a hostile access point) | The Mac's public key is in the QR code, which travels by camera, not the network. Without the Mac's private key nobody can derive the phone's keys: the phone's hello doesn't open for an impostor, and an impostor's messages don't open on the phone ("The other device didn't prove it has this pairing code"). Relaying unchanged bytes gains nothing. Tested: `a_man_in_the_middle_cant_read_or_answer`. |
| P3 | Someone photographs the QR code and pairs their own phone first | They hold the secret, so their hello opens. The Mac then shows *their* phone (name, model) and a code that doesn't match the user's phone, and sends nothing until Send. The offer is single use, so the user's own phone is refused ("already used"), which is itself the warning. |
| P4 | Guessing the code | 256-bit secret. A wrong hello counts as a failure; five withdraw the offer. |
| P5 | Replay of recorded frames | The offer is spent by the first valid hello, so a replayed hello is refused (`AlreadyUsed`); every later frame is bound to a per-direction counter, so replayed or reordered frames fail to open. Tested: `replays_are_refused`, `reordered_frames_are_refused`. |
| P6 | Tampering | AEAD tag over each frame and the transcript; a changed byte anywhere fails (`tampered_frames_are_refused`). |
| P7 | A stale or leaked QR code used later | Expires after 2 minutes (checked on the Mac); the key pair and secret are made fresh per offer and wiped when the offer ends. A later leak of the code reveals nothing about a finished pairing's traffic without the Mac's private key, which no longer exists. |
| P8 | Hostile hello content (huge lists, control characters, bidi overrides in the device name) | Frames are capped at 256 KB, account lists at 500, text cleaned and cut to 120 characters before the Mac shows it. |
| P9 | A phone build with a different Google client | The Mac sends those iOS-client tokens' accounts as "sign in"; the phone refuses any token minted by a client it doesn't hold (its own iOS client, or a desktop client that came with the tokens or from an earlier pairing). |
| P10 | Tokens or the desktop client secret in logs, crash reports or memory | Never logged (redacting `Debug` on every type holding one; logs name counts and addresses only). The vault JSON, desktop client secret, QR text, secret, private keys and derived keys are zeroized on drop; serde's intermediate copies are not guaranteed to be. On the phone the desktop client lives only in the Keychain, never in a file. |
| P12 | Downgrade: a forged or old code that leaves out the desktop client, or mixes versions | The version is in the QR text, the hello and the key-derivation labels; each side refuses any version but its own, with a message to update. A transfer without the desktop client only means browser sign-ins stay "sign in"; nothing else changes. |
| P13 | A hostile Mac (one the user confirmed) sends a desktop client to steer the phone | It's used only to refresh the tokens recorded with that same client id, never for sign-in, so it can't capture a sign-in on the phone. A Mac the user confirmed already holds the tokens it sends. Malformed ids or secrets (non-ASCII, over 200 characters) are dropped. |
| P14 | A hostile Mac (one the user confirmed) sends IMAP server settings that point the phone's password at its own server, or a Microsoft token for an account the user didn't mean to add | It already holds the password and token it sends, so it learns nothing new. The phone takes settings only for accounts in the transfer that come with their credential, only as a JSON object of at most 4 KB, checks them like a typed request (TLS or STARTTLS; no unencrypted server, which the Mac only allows for Proton Mail Bridge on itself), and refuses an address already on the phone with another provider. |
| P15 | IMAP passwords or Microsoft tokens in logs, crash reports or memory | Same as P10: `SecretString` (redacting `Debug`, wiped on drop) for `imapVault` and `microsoftVault`; `PasswordCredential` and `MicrosoftCredential` redact their `Debug`; logs carry counts and account ids only. On the phone they live only in the Keychain. |
| P16 | Downgrade to a version that leaves the IMAP and Microsoft accounts behind | Version 3 is bound into the QR text, the hello and the key derivation (P12); a version 2 device on either side is refused with "update both", so a transfer is never silently Gmail only. |
| P11 | Denial of service on the network (connect floods) | At most 16 hellos read at once, 10 seconds each; the offer still expires on time. A LAN attacker can stop a pairing, not subvert it. |

### Shared grants

After pairing, the Mac and the phone hold the same refresh token. Google
revokes a grant per user and client, so revoking it on one device signs out
both. The phone therefore never revokes an account that came from the Mac:
removing it there deletes the token from the phone's Keychain only
(`AuthManager::forget`). Removing the account on the **Mac** still revokes (as
it always has), which signs the phone out of it too; the phone then shows it
as needing sign-in. The same is true of any two devices signed in with the
same iOS client.

IMAP and Microsoft accounts share their credential the same way. Removing
one on the phone deletes the phone's copy only: an app password keeps
working on the Mac until it is deleted at the provider, and a Microsoft
public client has nothing to revoke (each device refreshes its own copy of
the token; Microsoft rotates refresh tokens without revoking the earlier
one, so the Mac's keeps working after the phone refreshes). Deleting the
app password at the provider, or signing the Microsoft account out of every
session there, stops both devices; the phone then shows the account as
needing sign-in (the IMAP password screen, or Microsoft's sheet).

## Not covered yet

- No Bonjour discovery: if the Mac's address changes between showing the code
  and the scan, start again.
- The Mac app's Local Network and firewall prompts: macOS asks for Local
  Network access for outgoing local connections; accepting connections should
  not need it, but if the application firewall is on, macOS asks whether
  Penguin may accept incoming connections once.
- Settings beyond account metadata (signatures, rules, snoozes) don't move.
- An IMAP account with a separate SMTP password moves it too (it's part of
  the vault entry); the phone has no screen to change it, only the password
  screen that replaces both.
