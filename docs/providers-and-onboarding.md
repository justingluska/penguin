# Providers and onboarding: Google verification, Microsoft, Yahoo, IMAP, and "type your email"

Research snapshot **2026-09-25**, checked against live docs and source code. This replaces `docs/providers-outlook-yahoo.md` (2026-09-24), which is now superseded; corrections to it are marked **[corrects old doc]**. Goal: open-source Penguin, where a user types an email address, Penguin detects the provider, and it runs the right sign-in with no Cloud console.

## Decision summary

1. **Google: bring your own client (decided 2026-09-25).** Penguin is released as open source only: every user or fork creates their own Google Cloud project and OAuth client (`docs/google-setup.md`), and nothing in the repo points at a maintainer-run project. A verified shared client (option (b)/(c) below) would cost about **$540–$1,800 a year** for a CASA Tier 2 assessment plus **4–8 weeks** of review, and Google has required CASA even of server-less desktop clients (Mimestream, Thunderbird for Android/K-9); the analysis is kept below for reference. Microsoft is bring-your-own too (each user registers their own Entra app); Yahoo, iCloud and Fastmail use app passwords.
2. **Microsoft: each user brings their own multitenant public client (no secret), using Graph** (superseded 2026-09-25 by point 1: nothing is committed; the original plan was one shared client with its ID in the repo, analysis kept below). Personal Outlook.com/Hotmail accounts work immediately. **Work (M365) accounts now need tenant-admin consent by default** (a Nov 2025 change). Only Apple Mail, Spark, eM Client, Thunderbird and two Android clients are exempt. Publisher verification is free and worth doing, but it does not lift that block. **[corrects old doc]**
3. **Yahoo/AOL, iCloud, Fastmail and everything else: IMAP+SMTP with an app password.** Yahoo mail OAuth is not self-serve. Apple offers no third-party mail OAuth at all. Fastmail's OAuth is available to partners only.
4. **Onboarding:** check a static domain table, then an MX lookup, then Thunderbird's autoconfig chain (ISP URL, then the ISPDB), then a guess. Always show a manual provider picker.
5. **Engineering:** about **25–35 agent-sessions** over **2–4 calendar weeks** with 3–4 parallel agents. That assumes the provider seam lands first (a serial step) and that the maintainer tests against real accounts. The hard parts are IMAP sync correctness, threading and identity, Microsoft's consent edge cases, and quota. The rest is mechanical.

---

## 1. Google

### 1.1 Which scopes Penguin needs, and whether any avoid "restricted"

The classification comes from Google's [Gmail scopes page](https://developers.google.com/workspace/gmail/api/auth/scopes):

- **Non-sensitive:** `gmail.labels`.
- **Sensitive:** `gmail.send`.
- **Restricted:** `https://mail.google.com/` (this also covers IMAP/SMTP XOAUTH2), `gmail.readonly`, `gmail.compose` (drafts), `gmail.modify`, `gmail.metadata`, `gmail.insert`, `gmail.settings.basic` and `gmail.settings.sharing`.

**No combination that can read mail avoids restricted status.** Even headers-only (`gmail.metadata`) is restricted, and so is drafts-only. Penguin reads, relabels, sends and drafts, so `gmail.modify` is the correct minimal scope. It covers all four. Keep requesting it, along with `openid email profile`. Calendar (`calendar.readonly`, `calendar.events`) and contacts scopes are only **sensitive** and can go through the same review.

"Built-in and web email clients that allow users to compose, send, read, and process email via a user interface" is the first approved Gmail use case ([Workspace API user data policy](https://developers.google.com/workspace/workspace-api-user-data-developer-policy)). Penguin fits it exactly.

### 1.2 What happens without verification (confirmed)

- **Testing** ([Manage app audience](https://support.google.com/cloud/answer/15549945?hl=en)): at most 100 listed test users. "Authorizations by a test user will expire seven days from the time of consent", and offline refresh tokens expire too. The exception for identity-only scopes doesn't apply to Penguin. [OAuth2 overview](https://developers.google.com/identity/protocols/oauth2) says the same.
- **In production but unverified:** users see a "Google hasn't verified this app" screen and must click Advanced → Go to … (unsafe). The cap is **"100 new users in total, after the app presents the unverified app screen"**. It applies over the project's lifetime and "cannot be reset or changed" ([Unverified apps](https://support.google.com/cloud/answer/7454865?hl=en)). Google explicitly allows personal-use apps under 100 users to stay unverified ([When verification is not needed](https://support.google.com/cloud/answer/13464323?hl=en)). That exception is why BYO works: every user's project is its own "personal app".
- Workspace admins can mark any client ID **Trusted**, whatever its verification status (same page). This helps companies, not the public.

### 1.3 What verification requires

From [Restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification):

- **Published branding.** This means an app name, logo, a **public homepage** on a domain you own, and a **privacy policy** that discloses how Google user data is accessed, used, stored and shared, including a Limited Use statement.
- **Domain ownership verified** in Google Search Console for every authorized domain.
- **A demo video** on YouTube (unlisted), in English. It must show the whole consent flow, the app name on the consent screen, **the client ID in the browser address bar**, and each restricted or sensitive scope being used. If the app uses several clients (Penguin has a Desktop client and an iOS sheet client), it must show each one.
- **A written justification** of why narrower scopes aren't enough, plus the permitted app type (email client).
- **Timing:** brand review "typically takes 2-3 business days". The restricted review "can potentially take several weeks".
- **Annual renewal:** apps "must be reverified for compliance and complete a security assessment at least every 12 months after your assessor's Letter of Assessment (LOA) approval date".

### 1.4 CASA: the "third-party server" wording versus what actually happens

Google's wording: "Every app that requests access to Google users' restricted data and **has the ability to access data from or through a third-party server** must go through a security assessment" (same page). The scopes page says: "If you store restricted scope data on servers (or transmit), then you must go through a security assessment."

On paper, a Penguin with no server is exempt. In practice, **do not count on it:**

- **Mimestream** is a macOS Gmail client whose tokens live in Keychain and never touch its servers. It had to do CASA anyway: "Google began extending this requirement to Mimestream (a non-cloud, traditional desktop client) as well." It passed **Tier 2** and renews every year ([Mimestream CASA post](https://mimestream.com/blog/casa-verified), [Mimestream security overview](https://mimestream.com/trust/security-and-privacy)). Mimestream does run a push relay that only sees opaque history IDs, so Google may have counted that.
- **Thunderbird for Android/K-9** did Tier 2 with NetSentries. The assessment took 9–17 days, including remediation ([tracking issue #8829](https://github.com/thunderbird/thunderbird-android/issues/8829)).
- **Tier** is decided by Google, based on risk ([CASA help](https://support.google.com/cloud/answer/13465431?hl=en)). Small apps get Tier 2: a self-scan followed by lab validation.
- **Cost:** TAC Security's Tier 2 plans run **$540 (basic), $720 (unlimited rescans) and $1,800 (enterprise)** a year ([pricing roundup](https://www.switchlabs.dev/post/casa-tier-2-tier-3-security-review-providers-pricing-and-the-cheapest-option), [overview](https://deepstrike.io/blog/google-casa-security-assessment-2025)). Tier 3 is where the "thousands" figure comes from. **Budget about $600–$1,800 a year.** If Google judges the app to have no server path, that line drops to $0.

**Realistic Google timeline for the maintainer:**

| Step | Time |
|---|---|
| Domain and site: homepage, privacy policy and Limited Use statement, e.g. on a `penguin` page of a domain he owns | ½ day |
| Verify the domain in Search Console | minutes |
| New **production** project "Penguin" (separate from any personal test project), with Desktop and iOS clients | ½ day |
| Record and upload the video | ½ day |
| Brand review | 2–3 business days |
| Restricted review, back-and-forth | 2–6 weeks |
| CASA Tier 2, including fixes | 1–3 weeks, can overlap the review |

Total: about **4–8 weeks of calendar time, 2–4 days of the maintainer's time, and about $540–$1,800 a year**. Until approval, official builds can ship with the BYO flow.

### 1.5 Baked-in client ID and secret in an open-source app

- **Google's technical position:** "Installed apps are distributed to individual devices, and it is assumed that these apps cannot keep secrets" ([native-app OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)). That doc now lists `client_secret` as *Optional* at the token endpoint when PKCE is used. Penguin's iOS-type client already has no secret.
- **Google's legal position:** the [Google APIs Terms §5b](https://developers.google.com/terms) say "Developer credentials (such as passwords, keys, and **client IDs**) … You will keep your credentials confidential and make reasonable efforts to prevent and discourage other API Clients from using your credentials. **Developer credentials may not be embedded in open source projects.**"
- **What others do:**
  - **Thunderbird desktop** commits its Google client ID *and* secret in [`OAuth2Providers.sys.mjs`](https://raw.githubusercontent.com/mozilla/releases-comm-central/master/mailnews/base/src/OAuth2Providers.sys.mjs). It also commits IDs for Microsoft, Yahoo, AOL and Fastmail, and uses IMAP with `https://mail.google.com/`.
  - **Mailspring** also keeps `GMAIL_CLIENT_ID`/`GMAIL_CLIENT_SECRET` in its public onboarding code, with a `127.0.0.1` loopback ([source](https://github.com/Foundry376/Mailspring/blob/master/app/internal_packages/onboarding/lib/onboarding-helpers.ts)).
  - **Mimestream** is closed source and verified.
  - **GNOME Online Accounts/Evolution** take the Google client ID as a build-time option, so distributions inject their own.
  - This compliant pattern is also how Chromium handles Google API keys.
- **Recommendation:** keep the verified client out of git. `release.yml` injects it from a GitHub secret, like the updater key. A binary's credentials can always be extracted, and Google accepts that for installed apps. The ToS problem is only about the *repo*.

### 1.6 One project for all users, and forks

One verified project serves every user of the official binary. Verification, the user cap and quota all belong to the project. **Quota becomes shared**: 1,200,000 units per minute across the project and 6,000 per user per minute on new projects (see `google-setup.md`). That is fine per user, but ask for a raise before launch.

Forks must not reuse the client. The ToS requires "reasonable efforts to prevent and discourage" it, and the consent screen would show *Penguin's* name for someone else's code. Put this in the README and CONTRIBUTING.

If a fork abuses the client, Penguin's only lever is rotation. Refresh tokens are bound to the client that issued them, so rotating forces every user to reconnect. Microsoft's old Thunderbird ID (`08162f7c…`) was disabled in Aug 2024 after wide third-party reuse, which forced a rotation ([issue](https://github.com/simonrob/email-oauth2-proxy/issues/267)).

### 1.7 Options matrix

| | (a) BYO project per user (today) | (b) Verified client only | **(c) Hybrid (recommended)** |
|---|---|---|---|
| User friction | About 10 minutes of Cloud console, an "unverified" screen, and Workspace admin approval per organization | One click | One click by default; BYO for source builds and forks |
| the maintainer's cost | $0 | $540–$1,800 a year, 4–8 weeks, annual renewal | Same as (b) |
| ToS | Clean | Clean if kept out of git | Clean if kept out of git |
| Single point of failure | None | Revocation or suspension strands everyone | Revocation strands official users, but BYO is the escape hatch |
| Code | Exists | Build-time injection | Injection plus precedence: env or user JSON > built-in > onboarding BYO wizard |

Precedence for (c): `PENGUIN_GOOGLE_CLIENT_JSON`, then the user's config-dir JSON, then the client compiled in, then the BYO wizard. Record the issuing client with each token, as Penguin already does for the iOS sheet, so switching clients never breaks existing refresh tokens.

**Gmail via app password.** Personal Gmail accounts with 2-Step Verification can still create app passwords at `myaccount.google.com/apppasswords`. Work/school, security-key-only and Advanced Protection accounts can't ([help](https://support.google.com/accounts/answer/185833?hl=en)). The generic IMAP provider therefore reaches personal Gmail with no Google verification at all. It loses Gmail API features, and Google discourages app passwords. Offer it only as a last resort, never as the default.

---

## 2. Microsoft (Outlook.com, Hotmail, Live and Microsoft 365)

**Protocol: use Graph, not IMAP.**
- Graph supports personal accounts and work/school accounts. It provides `categories`, `conversationId`, MIME get/send and drafts, plus per-folder delta ([mail overview](https://learn.microsoft.com/en-us/graph/outlook-mail-concept-overview), [MIME send](https://learn.microsoft.com/en-us/graph/outlook-send-mime-message)).
- **Delta works one folder at a time** (`/me/mailFolders/{id}/messages/delta`). There is no delta across the whole mailbox ([message: delta](https://learn.microsoft.com/en-us/graph/api/message-delta)). **[corrects old doc]**
- Throttling is **10,000 requests per 10 minutes and 4 concurrent requests per app per mailbox** ([limits](https://learn.microsoft.com/en-us/graph/throttling-limits)).
- IMAP/SMTP XOAUTH2 still works for M365 and Outlook.com (`https://outlook.office.com/IMAP.AccessAsUser.All`, `…/SMTP.Send`, `offline_access`) ([doc](https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth)). Keep it as a fallback only.
- Outlook.com stopped accepting passwords for IMAP on **2024-09-16**. Exchange Online SMTP AUTH Basic will be **off by default at the end of Dec 2026** ([timeline](https://techcommunity.microsoft.com/blog/exchange/updated-exchange-online-smtp-auth-basic-authentication-deprecation-timeline/4489835)).

**Scopes:** `openid profile email offline_access User.Read Mail.ReadWrite Mail.Send` (add `MailboxSettings.Read` for signature and time zone). Thunderbird's Graph set also adds `MailboxFolder.ReadWrite`.

**Consent: the big change. [corrects old doc]**
- The default tenant setting, "Let Microsoft manage your consent settings", now **blocks user consent** to `Mail.Read`, `Mail.ReadWrite`, `Mail.ReadBasic`, `MailBoxFolder.*`, `MailBoxSettings.*`, `Calendars.*` and `IMAP/EWS/POP/EAS.AccessAsUser.All` for third-party apps. Only an allowlist is exempt: Apple Mail, Spark, eM Client, Thunderbird and two Android clients ([Manage app consent policies](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies), [MC1163922](https://mc.merill.net/message/MC1163922), rolled out Oct–Nov 2025). `Mail.Send` is not on the blocked list.
- **Effect:** a new M365 user sees "Need admin approval" unless their admin grants consent once for the whole tenant. Personal Microsoft accounts are unaffected. There is no public process for joining the allowlist, so asking Microsoft is a long shot.
- **UX:** catch `AADSTS65001` and consent-required errors. Show a "Send this link to your IT admin" screen with `https://login.microsoftonline.com/{tenant}/adminconsent?client_id=<id>`.

**Publisher verification** ([overview](https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview)):
- **Free**, and done "in minutes" once the prerequisites exist. It needs:
  - a verified **Microsoft AI Cloud Partner Program** account at the partner-global level (enrolled as a business);
  - the app registered with a **work account in an Entra tenant**, not a personal MSA;
  - a publisher domain that isn't `*.onmicrosoft.com` and matches the Partner Program email domain or a DNS-verified domain;
  - MFA, and the App Admin + Partner Admin roles.
- It adds the blue badge. It also unblocks tenants that use "verified publishers only" or risk-based step-up consent, since apps registered after Nov 2020 without verification can be blocked there. It does **not** bypass the managed mail block above.

**Redirect:** use the "Mobile and desktop applications" platform with `http://localhost`. The port is ignored for localhost matching ([redirect rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)). Microsoft prefers `127.0.0.1`, but the portal text box rejects `http://127.0.0.1`, so that form needs a manifest edit. No query strings are allowed when personal accounts are in the audience. Use the `/common` authority, PKCE, and no secret. The client ID can live in the repo, as Thunderbird's `9e5f94bc-…` does, since it's a public client with nothing secret.

**the maintainer's click-steps (about 30 minutes, plus Partner Center vetting, which is typically days):**
1. Sign in at <https://entra.microsoft.com> with a **work account in a tenant the publisher owns**. Registering apps outside a directory is deprecated. If there is no tenant, a free Azure account creates one.
2. **Entra ID → App registrations → New registration.** Name: `Penguin`. Supported account types: the option covering **any organizational directory (multitenant) and personal Microsoft accounts**. The portal wording changes between versions. Redirect URI: platform **Public client/native (mobile & desktop)**, `http://localhost`. Click **Register**.
3. **Authentication:** confirm the mobile/desktop platform is listed. Leave **Allow public client flows = No**; auth code + PKCE doesn't need it.
4. **API permissions → Add a permission → Microsoft Graph → Delegated:** `openid`, `profile`, `email`, `offline_access`, `User.Read`, `Mail.ReadWrite`, `Mail.Send`, `MailboxSettings.Read`. Do **not** click "Grant admin consent". That only covers his own tenant.
5. **Branding & properties:** add the logo, home page, terms and privacy URLs, and set the publisher domain to a DNS-verified custom domain.
6. Copy the **Application (client) ID** into Penguin's provider config.
7. Optional (recommended): enroll in the AI Cloud Partner Program at <https://partner.microsoft.com/membership>. After it's verified, go to **App registration → Branding & properties → Publisher verification → Add Partner One ID**.

---

## 3. Yahoo/AOL, iCloud, Fastmail and generic IMAP

| Provider | Authentication for Penguin | Where the user makes the password | Servers |
|---|---|---|---|
| **Yahoo** | App password. OAuth (`mail-r`/`mail-w`) is "not available for self-served setup"; commercial access is by application form or `mail-api@yahooinc.com`, and Yahoo can revoke it any time ([Yahoo developer access](https://senders.yahooinc.com/developer/developer-access/)). Thunderbird has an approved client. | login.yahoo.com → **Account Security** → under **"External connections"** click **Create app password** → name it → **Generate** ([Yahoo help](https://help.yahoo.com/kb/SLN15241.html)). Use a browser that has been signed in for days, not incognito. App passwords survive password changes and must be deleted to revoke. | `imap.mail.yahoo.com:993` SSL, `smtp.mail.yahoo.com:465` SSL (or 587 STARTTLS) |
| **AOL** | Same Yahoo platform and same rules. | login.aol.com → Account Security → Create/Generate app password ([AOL help](https://help.aol.com/articles/Create-and-manage-app-password)) | `imap.aol.com:993`, `smtp.aol.com:465` |
| **iCloud** (`icloud.com`, `me.com`, `mac.com`, and custom domains with MX `mx0x.mail.icloud.com`) | App-specific password. There is no third-party OAuth. | account.apple.com → **Sign-In and Security** → **App-Specific Passwords** → Generate. Up to 25 are allowed ([Apple](https://support.apple.com/en-us/102654)). | `imap.mail.me.com:993` SSL; `smtp.mail.me.com:587` STARTTLS. IMAP username is usually the address *without* the domain; SMTP username is the full address ([Apple](https://support.apple.com/en-us/102525)). |
| **Fastmail** | App password over IMAP. OAuth clients are "registered manually by contact with Fastmail" ([Fastmail OAuth](https://www.fastmail.com/for-developers/oauth/)). JMAP with an API token is an option for later. | Settings → **Privacy & Security** → **Connected apps & API tokens** → **Manage app passwords and access** → **New app password** ([Fastmail](https://www.fastmail.help/hc/en-us/articles/360058752854-App-passwords)) | `imap.fastmail.com:993`, `smtp.fastmail.com:465/587` |
| **Generic** | Password or app password. Offer OAuth only where Penguin has a registered client. | — | From autoconfig (§4), or entered by hand |

Store app passwords in Keychain beside the OAuth tokens and never log them. Deep-link the user to each "create app password" page from onboarding.

**Correction:** the old doc said Yahoo lacks CONDSTORE and that Fastmail Basic has no IMAP. Neither claim was verified, so they are dropped. **Probe `CAPABILITY` at runtime; never hardcode what a server supports.**

---

## 4. Detecting the provider from an email address

This mirrors Thunderbird's account-setup code, [`FetchConfig.sys.mjs`](https://raw.githubusercontent.com/mozilla/releases-comm-central/master/mail/components/accountcreation/modules/FetchConfig.sys.mjs).

1. **Static domain table, offline and instant:**
   - Google: `gmail.com`, `googlemail.com`.
   - Microsoft personal: `outlook.*`, `hotmail.*`, `live.*`, `msn.com`, `windowslive.com`. The ISPDB `outlook.com` entry lists about 40 country variants.
   - Yahoo: `yahoo.*`, `ymail.com`, `rocketmail.com`, `myyahoo.com`.
   - AOL: `aol.com`.
   - iCloud: `icloud.com`, `me.com`, `mac.com`.
   - Fastmail: `fastmail.com`, `fastmail.fm`, and its other domains.
   - Generate the table from ISPDB snapshots so no one maintains it by hand.
2. **MX lookup** for custom domains, via `hickory-resolver` or the system resolver:
   - `smtp.google.com` (the single record used since 2023) or `aspmx.l.google.com` and its `alt*`, or `*.googlemail.com` → **Google Workspace** (Gmail API).
   - `*.mail.protection.outlook.com` → **M365** (Graph). `*.olc.protection.outlook.com` → Outlook.com.
   - `*.yahoodns.net` → Yahoo.
   - `mx0[12].mail.icloud.com` → iCloud.
   - `*.messagingengine.com` → Fastmail.
   - Anything else: look up the MX's base domain in the ISPDB.
   - **Caveat:** tenants behind Proofpoint or Mimecast hide their real provider. There, fall back to the picker. An optional probe is Autodiscover v2 JSON (`https://autodiscover-s.outlook.com/autodiscover/autodiscover.json?Email=<addr>&Protocol=AutodiscoverV1`), but it sends the full address to Microsoft, so ask first.
3. **Autoconfig, for IMAP hosts**, in Thunderbird's order:
   1. `https://autoconfig.<domain>/mail/config-v1.1.xml`
   2. `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`
   3. the ISPDB at `https://autoconfig.thunderbird.net/v1.1/<domain>`. Live check: `gmail.com`, `outlook.com`, `yahoo.com`, `icloud.com` and `aol.com` return 200 and `fastmail.com` returns 404, so the static table covers Fastmail.
   4. the ISPDB again, for the MX domain.

   Thunderbird adds `?emailaddress=` for the ISP's own URLs. Penguin should send only the domain. Parse the `<authentication>` values (`OAuth2`, `password-cleartext`) to choose the sign-in method.
4. **Guess:** `imap.<domain>` or `mail.<domain>` on 993, then SMTP on 465/587. Verify with a live login, then show an editable "Manual settings" form.

Always show buttons under the email field: **Google, Microsoft, iCloud, Yahoo, Other (IMAP)**. This matches Apple Mail's provider picker and covers wrong guesses. Detection results route to three flows: **OAuth browser flow** (Google, Microsoft), **app-password screen with deep link** (Yahoo, AOL, iCloud, Fastmail, and personal Gmail as a fallback), and **manual IMAP**.

---

## 5. Engineering plan (agent-driven)

### 5.1 What was Gmail-shaped before phase 0

Phase 0 has landed (2026-09-25): the seam, `Account.provider`, the opaque
cursor and the dispatch are in place; `docs/PROVIDERS-IMPL.md` is the brief
for the IMAP and Microsoft agents. The list below is the starting point it
replaced.

- `Account` has no `provider`, and `AccountId` is the lowercased email.
- `Message.id` and `thread_id` are Gmail IDs.
- Flags are labels (`UNREAD`, `STARRED`, `INBOX`, `CATEGORY_*`).
- `MailboxView` follows Gmail's label set.
- `SyncCursor.history_id: Option<u64>` is stored in `sync_cursors.history_id`, and `backfill_page_token` is a Gmail page token.
- `to_gmail_query` and `search_server` sit in `penguin-gmail/src/sync_window.rs`.
- **About 20 files in `apps/desktop/src-tauri/src` import `penguin_gmail` directly.**
- Calendar, contacts, avatars and send-as are Google-only. Keep them Google-only in v1 of the other providers.
- Local FTS search, the renderer, rules, OTP and unsubscribe are already provider-neutral.

### 5.2 Model changes (phase 0, serial, one lead agent)

| Concept | Change |
|---|---|
| Provider | Add `Account.provider` (`gmail`, `graph`, `imap`) and `auth` (oauth client ref or app password). Migrate existing rows to `gmail`. Mirror in `types.ts`. |
| Message identity | Keep a stable Penguin id. Gmail: the Gmail id. Graph: `immutableId` (send `Prefer: IdType="ImmutableId"`, or IDs change on move). IMAP: `OBJECTID` `EMAILID` where the server supports it (RFC 8474), otherwise a hash of Message-ID plus date and size, with a `(folder, uidvalidity, uid)` locator table. |
| Threads | Gmail: native. Graph: `conversationId`. IMAP: JWZ over `Message-ID`/`In-Reply-To`/`References`, with a subject fallback only inside a time window. Store as `thread_id`. |
| Labels vs folders | Keep `label_ids`. Map IMAP SPECIAL-USE folders (`\Inbox \Sent \Drafts \Trash \Junk \Archive \All`) to the system labels, and other folders to `Label(path)`. Map `\Seen` and `\Flagged` to the absence of UNREAD and to STARRED. Graph: well-known folders map to system labels, categories to labels. "Archive" means a move to the Archive folder, not removing INBOX. |
| Cursor | Keep the Gmail fields. Add `provider_state TEXT` (JSON). Graph: a deltaLink per folder. IMAP: per folder `{uidvalidity, uidnext, highestmodseq}`. |
| Server search | Add a `ProviderSearch` trait. Gmail keeps `q=`. Graph uses `$search` (KQL-like, with limits on combining it with `$filter`). IMAP uses `UID SEARCH` (weak; body search is often unsupported). Local FTS stays primary. |
| Send and drafts | Build RFC 5322 once with the existing compose plus `mail-builder`. Gmail: raw. Graph: `sendMail` with MIME, and drafts via `POST /me/messages`. IMAP: SMTP (`lettre`), then `APPEND` to Sent unless the server auto-saves; drafts via APPEND+EXPUNGE when replacing. |
| Seam | A `MailProvider` trait (backfill, incremental, modify, send, drafts, body/attachment fetch, labels/folders, search), with desktop commands dispatched through a registry keyed by account. Include a **fake provider plus a conformance test suite** that every provider must pass. |

### 5.3 Phases in agent-sessions

One session means one focused agent run with review, roughly half a day to a day of wall-clock time.

| Phase | Work | Parallel? | Sessions | Kind |
|---|---|---|---|---|
| 0 | Schema migration, `provider` field, `provider_state`, `MailProvider` trait, registry, and refactoring the 20 desktop call sites; Gmail must not regress (existing tests plus the conformance suite on a fake provider) | **Serial, blocks everything** | 4–6 | Moderate; wide diff, needs careful review |
| 1a | Onboarding: detection module (static table, MX, autoconfig parser, guesser), email-first UI, app-password screens with deep links, Keychain storage, admin-consent screen | After 0, in parallel | 3–4 | Mechanical |
| 1b | IMAP/SMTP crate: connection pool, SPECIAL-USE mapping, UID backfill (newest first, `BODY.PEEK` only), CONDSTORE/QRESYNC incremental with a UID-diff fallback, IDLE on INBOX plus polling others, JWZ threading, flag/move/archive, SMTP plus Sent append, drafts, IMAP SEARCH | After 0, 2 agents | 8–12 | **Hard** |
| 1c | Graph crate: PKCE on `/common` (reusing `auth/loopback.rs` and `pkce.rs`), per-folder delta and immutable IDs, 410 Gone on delta → resync, `conversationId`, categories, MIME send and drafts, `$search`, 429/`Retry-After` with 4-concurrent limiting | After 0, 1 agent | 5–7 | Medium-hard |
| 1d | Google hybrid: build-time client injection in `release.yml`, precedence chain, "Penguin (verified)" versus BYO in Settings | After 0 | 1 | Mechanical |
| 2 | Integration tests: Dovecot/GreenMail/Stalwart in containers for IMAP (UIDVALIDITY reset, expunge, and move scenarios); recorded Graph fixtures (fictional `.example` data only); plus real-account testing by the maintainer on outlook.com, an M365 dev tenant, iCloud, Yahoo and Fastmail | Partly parallel | 4–5 | Hard to automate |

**Total: about 25–35 sessions, about 2–4 calendar weeks** with 3–4 concurrent agents. The critical path is phase 0, then IMAP, then real-account testing.

### 5.4 Hard and risky versus mechanical

**Hard:**
- **IMAP sync correctness.** UIDVALIDITY changes force a folder rescan. Detecting deletions without QRESYNC needs a `UID SEARCH ALL` diff. A move changes the UID, so identity must survive it. The same message can sit in several folders (Gmail-style servers, Sent plus Inbox for self-mail). Everything must be idempotent across crashes.
- **Threading.** JWZ merges and splits across folders, and mailers that drop `References` break chains. Subject-only merging creates false threads. Microsoft's `conversationId` disagrees with JWZ, so don't mix them.
- **Graph gotchas.** Mutable IDs, delta tokens expiring (410), a delta per folder so N folders mean N cursors, and 4 concurrent requests per mailbox.
- **Microsoft consent edge cases.** Admin consent (65001), conditional-access or compliant-device requirements (`AADSTS53000`, which Penguin cannot satisfy), `/common` versus `/consumers` endpoints, token revocation on password reset, and multi-geo tenants.
- **Quota and politeness.** Yahoo and iCloud throttle aggressive parallel IMAP connections, so cap at 2–4 per account. Handle the Graph 429s.
- **Outbox semantics.** Send-later and undo-send must work the same across three send paths, and Sent-copy duplication must be avoided.

**Mechanical:** detection and autoconfig, onboarding screens, the Keychain for app passwords, MIME build and parse (already done), label-to-flag mapping tables, the Google build-time injection, and settings UI.

## Sources not linked inline

- Thunderbird ISPDB entries (fetched live): `https://autoconfig.thunderbird.net/v1.1/{gmail.com,outlook.com,yahoo.com,icloud.com,aol.com}`.
- Crate versions checked on crates.io, 2026-09-25: `async-imap` 0.11.3, `imap-codec` 1.0.0, `imap-next` 0.3.4, `lettre` 0.11.23, `mail-parser` 0.11.9, `mail-builder` 1.0.0, `hickory-resolver` 0.26.3, `jmap-client` 0.4.2. Prefer `async-imap`, which is battle-tested in Delta Chat and has IDLE, CONDSTORE and QRESYNC through raw commands. `imap-next`/`imap-codec` are the stricter, newer choice.

## Gmail quick setup over IMAP (researched 2026-09-25)

Offer **"Gmail – quick setup (no Google Cloud project)"** as a second option under Google, riding the IMAP provider: `imap.gmail.com:993` / `smtp.gmail.com:465`, app password from myaccount.google.com/apppasswords (needs 2-Step Verification). The BYO-client Gmail API path stays the recommended default.
- **Personal @gmail.com only.** Google Workspace turned off basic auth, app passwords included, for IMAP/SMTP from 2025-03-14 ([transition](https://support.google.com/a/answer/14114704)); Advanced Protection blocks app passwords ([APP FAQ](https://support.google.com/accounts/answer/7539956)). Detect Workspace (MX on a non-gmail.com domain) and show the OAuth-only explanation instead of failing.
- **Keep Gmail semantics** with the Gmail IMAP extensions: `X-GM-LABELS` (labels), `X-GM-THRID` (thread id), `X-GM-MSGID` (stable message id), `X-GM-RAW` (Gmail search syntax) ([extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)). Sync from `[Gmail]/All Mail` and map labels, not folders. IMAP has been always-on since Jan 2025.
- **Limits:** about 2,500 MB/day IMAP download and 500 MB/day upload ([bandwidth](https://support.google.com/a/answer/1071518)); 500 sends/day for personal accounts. A large first sync can span days; pace it and show progress honestly.
- **Disclose in the UI:** an app password grants full account access, can't be scoped, and can be revoked in the Google Account; sync is slower than the API path.
- **Not viable:** reusing another app's OAuth client (ToS), an unverified shared client (100-user cap, 7-day tokens), the device flow (no Gmail scopes), Apps Script/Takeout.
