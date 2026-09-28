# Google setup: bring your own OAuth client

Checked against Google’s live documentation on **2026-09-23**. Allow about **10 minutes for Google configuration and installing the client JSON**, with additional time for administrator changes or propagation. Mailbox backfill runs afterward.

**Recommended:** one project, one **Desktop app** client, **External → In production**, used only by your own Google accounts. Choose **Internal** instead only if every account belongs to the same Workspace organization. Penguin connects directly from the Mac to Google; there is no Penguin server.

## 1. Choose the organization, create the project, enable Gmail — 2 minutes

| Your accounts | Project location | Audience |
| --- | --- | --- |
| All in one Workspace organization, even if it has multiple domains | That organization | **Internal** |
| Several independent Workspace organizations, or any `gmail.com` account | Your primary organization, one you control | **External** |
| Prefer personal ownership independent of Workspace | Sign in with your personal Google account; choose **No organization** | **External** |

Internal is bounded by the **project’s parent Workspace/Cloud Identity organization**. Owning several domains or being administrator of several organizations does not combine them. Adding someone to Cloud project IAM does not make their Google account an internal Workspace user. Google documents the [organization boundary and `org_internal` error](https://support.google.com/cloud/answer/15549945?hl=en). A personal account can create a project under [No organization](https://codelabs.developers.google.com/set-up-and-navigate-your-first-google-project).

1. Open [Google Cloud Console](https://console.cloud.google.com/), signed in as the account that will own/manage the project.
2. Click the project selector in the top bar → **New project**. Alternatively: **☰ → IAM & Admin → Create a Project**.
3. Set **Project name** to `Penguin Private`. Note the generated **Project ID**.
4. Under **Organization/Location**, use **Browse** to select the organization chosen above, or **No organization** for personal ownership. Click **Select → Create**.
5. Select `Penguin Private` in the top project selector once creation finishes. These are Google’s [project creation steps](https://developers.google.com/workspace/guides/create-project).
6. Open **☰ → APIs & Services → Library**, search `Gmail API`, open **Gmail API**, and click **Enable**. Confirm the project selector still shows `Penguin Private`. [Enable Workspace APIs](https://developers.google.com/workspace/guides/enable-apis).

Use an organization you control long term; its administrators control the project. If project creation is denied, check the selected account and its project-creation permissions with that organization’s administrator.

## 2. Configure Google Auth Platform — 2 minutes

The current console uses **Google Auth Platform**, with **Branding**, **Audience**, **Data Access**, and **Clients** pages. Older instructions call this the “OAuth consent screen.”

1. Open **☰ → Google Auth Platform → Branding**. Click **Get Started** if prompted.
2. **App Information:** enter `Penguin Private`; choose your **User support email** → **Next**.
3. **Audience:** select **External** for the recommended cross-organization setup, or **Internal** for the single-organization case → **Next**.
4. **Contact Information:** enter an email address you monitor → **Next**.
5. **Finish:** review and accept **Google API Services: User Data Policy** → **Continue → Create**. Leave the optional logo unset for this private setup.
6. If staying in External/Testing temporarily, open **Audience → Test users → Add users**, enter each Google sign-in address you will use, and **Save**. Skip this if publishing before the first sign-in. Internal does not need a test-user list. [Google’s current consent configuration guide](https://developers.google.com/workspace/guides/configure-oauth-consent).

## 3. Add exactly these scopes — 1 minute

1. Open **Google Auth Platform → Data Access → Add or Remove Scopes**.
2. Select `openid`, `https://www.googleapis.com/auth/userinfo.email`, and `https://www.googleapis.com/auth/userinfo.profile` in the table. These identity permissions correspond to Penguin’s `openid email profile` request.
3. Find and select `https://www.googleapis.com/auth/gmail.modify`. If absent, use **Manually add scopes**, paste that full URL, then **Add to Table**.
4. Click **Update → Save**. Confirm Gmail modify is under restricted scopes and the identity scopes are under non-sensitive scopes. Google documents [scope selection and manual entry](https://support.google.com/cloud/answer/15549135?hl=en). For Internal apps, Google does not require listing scopes in the consent configuration; Penguin still requests them at authorization.

| Scope sent by Penguin | What you permit |
| --- | --- |
| `https://www.googleapis.com/auth/gmail.modify` | Read mail, including bodies; compose drafts and send mail; change labels, archive, mark read/unread, star, and move mail to Trash. It does **not** permit immediate permanent deletion bypassing Trash. Classified **restricted**. [Gmail scopes](https://developers.google.com/workspace/gmail/api/auth/scopes). |
| `openid` | Authenticate the Google account using OpenID Connect, including its stable account identifier. |
| `email` | Obtain the account’s email address and email-verification status, so Penguin can identify the mailbox. |
| `profile` | Obtain basic profile information such as display name and picture when available. |

The three identity scopes do not authorize mailbox access. See Google’s [OpenID Connect scope definitions](https://developers.google.com/identity/openid-connect/openid-connect#authenticationuriparameters).

## 4. External only: publish before daily use — 30 seconds

1. Open **Google Auth Platform → Audience**.
2. Under **Publishing status**, click **Publish app**, confirm, and check that the status becomes **In production**. This publishes the OAuth configuration; it does not distribute Penguin or publish the mailbox.
3. For this personal deployment, do not start a verification submission merely to clear the expected warning. Production status and verification are separate.

Google’s exact wording on Testing is:

> “Authorizations by a test user will expire seven days from the time of consent.”

Offline refresh tokens expire too. The identity-only exception does not apply because Penguin requests Gmail access. Google defines production status as occurring “after selecting the Publish app button.” [Manage App Audience](https://support.google.com/cloud/answer/15549945?hl=en).

Google explicitly permits personal-use apps with fewer than 100 users to skip verification:

> “you and your limited number of users can continue using the app without going through verification”

That exception allows proceeding past the warning. It supports this private **External/In production/unverified** setup. [When verification is not needed](https://support.google.com/cloud/answer/13464323?hl=en).

Expect **“Google hasn’t verified this app.”** Google states the default unverified-app cap is “100 new users in total, after the app presents the unverified app screen.” This is separate from Testing’s test-user list and applies over the project’s lifetime; budget the three distinct Google accounts as three users. Google can adjust quotas based on risk. [Unverified-app cap](https://support.google.com/cloud/answer/7454865?hl=en), [lifetime rule](https://support.google.com/cloud/answer/15549945?hl=en).

**In production does not retain the test-user allowlist as an access restriction.** Keep the app and credentials private. If an account already authorized while Testing, run Penguin’s sign-in again after publishing to obtain a fresh refresh token; do not assume the old token’s expiry changed. Production tokens can still be revoked or expire for the reasons below.

Internal apps need neither public verification nor the unverified-app cap, but still need any applicable administrator approval. [Internal-use exception](https://support.google.com/cloud/answer/13464323?hl=en).

## 5. Create the Desktop client and install its JSON — 2 minutes

1. Open **Google Auth Platform → Clients → Create Client**.
2. Set **Application type → Desktop app**. Name it `Penguin Private Mac` → **Create**. [Desktop client instructions](https://developers.google.com/workspace/guides/create-credentials#desktop-app).
3. In the creation dialog, click **Download JSON** immediately. Also copy the **Client ID**, ending in `.apps.googleusercontent.com`, for Workspace admin approval. Google now makes the full secret downloadable only at creation; preserve this download. [Client-secret visibility](https://support.google.com/cloud/answer/15549257?hl=en).
4. Rename the downloaded file in Downloads to `google-oauth-client.json`. Keep its contents unchanged; a Desktop client JSON has an `installed` object.
5. In Terminal, install it outside the repository:

```sh
mkdir -p "$HOME/Library/Application Support/co.gluska.penguin"
install -m 600 "$HOME/Downloads/google-oauth-client.json" \
  "$HOME/Library/Application Support/co.gluska.penguin/google-oauth-client.json"
```

Penguin’s configuration lookup is:

1. **`PENGUIN_GOOGLE_CLIENT_JSON`**, when set: a path to the JSON file, not the JSON contents.
2. Otherwise **`<app config dir>/google-oauth-client.json`**, which on macOS is **`~/Library/Application Support/co.gluska.penguin/google-oauth-client.json`**.

For an alternate location, use an absolute path and launch Penguin from the same Terminal environment:

```sh
export PENGUIN_GOOGLE_CLIENT_JSON="$HOME/private-config/penguin/google-oauth-client.json"
```

For Finder/Dock launches, prefer the default file location; a Terminal export is not automatically inherited. Unset an old override with `unset PENGUIN_GOOGLE_CLIENT_JSON` if it points at a different client.

These paths and precedence follow [Penguin’s OAuth configuration contract](../crates/penguin-gmail/src/auth.rs) and [desktop identifier](../apps/desktop/src-tauri/tauri.conf.json). Refresh tokens belong in macOS Keychain, separate from this JSON. **Never commit the JSON, tokens, or an environment file containing credentials**, even to a private repository. [Google credential handling guidance](https://developers.google.com/identity/protocols/oauth2/policies).

There is no redirect-URI entry to configure for this Desktop client. Penguin uses the system browser, PKCE, and a temporary **`http://127.0.0.1:<random-port>`** listener. Desktop loopback remains supported; no public callback domain or fixed localhost port is needed. [Installed-app OAuth](https://developers.google.com/identity/protocols/oauth2/native-app#redirect-uri_loopback).

## 5b. Optional: sign in inside Penguin (iOS client) — 2 minutes

By default Penguin signs in through your browser and leaves a tab behind. With a second, **iOS**-type client, Penguin uses the macOS system sign-in sheet (ASWebAuthenticationSession) instead. It's a Safari window that Penguin opens and closes itself, and it reuses your Safari Google login. Google's own macOS sign-in library works the same way. Embedded web views are blocked by Google (`disallowed_useragent`), so Penguin never uses one.

1. In the **same project**, open **Google Auth Platform → Clients → Create client**.
2. Set **Application type → iOS**. Name it `Penguin Private Mac (sheet)`.
3. **Bundle ID:** `co.gluska.penguin`. Leave App Store ID and Team ID empty. → **Create**.
4. Copy the **Client ID** (ends in `.apps.googleusercontent.com`). An iOS client has **no secret**; there's nothing else to download. (The optional `.plist` download also works: Penguin reads `CLIENT_ID` from it.)
5. Give it to Penguin, either:
   - in **Settings → Accounts → Sign-in sheet** (or the onboarding step): paste the client ID, or
   - in Terminal:

```sh
printf '{"client_id":"%s"}' "PASTE-CLIENT-ID.apps.googleusercontent.com" > /tmp/ios.json
install -m 600 /tmp/ios.json \
  "$HOME/Library/Application Support/co.gluska.penguin/google-oauth-ios-client.json" && rm /tmp/ios.json
```

Notes:

- **No redirect URI or URL-scheme setup.** The redirect is `com.googleusercontent.apps.<client-id-prefix>:/oauth2redirect`, the reversed client ID Google assigns to every iOS client. The system sheet catches it itself, so nothing is registered in Penguin's Info.plist.
- The scopes and publishing status (sections 3–4) belong to the project, so the new client shares them.
- **Workspace approval is per client ID.** In each organization from section 6, also approve the iOS client's ID, or its sign-ins are blocked there even though the desktop client works.
- **Existing accounts keep working.** A Google refresh token only refreshes with the client that issued it. Penguin records the issuing client with every token, so accounts connected through the browser keep using the desktop client. **Reconnect** an account (or connect contact photos or calendar) to move it to the sheet and the iOS client. Keep the desktop client and its JSON either way; it's the fallback.
- Closing the sheet cancels sign-in. If the sheet can't open, remove the iOS client (clear the field, or delete that file) and sign in through the browser.

## 5c. Optional: Google Calendar — 1 minute

By default, adding an account asks for read-only calendar access in the same Google consent as Gmail ("Also connect Google Calendar (read-only)" under the sign-in button; the same switch is in Settings → Calendar as **Connect calendar when adding accounts**). Accounts added earlier connect with Settings → Calendar → Connect calendar. Every calendar permission is a separate checkbox on Google's screen and is optional: untick it and the account signs in with mail only.

The project needs these steps for the calendar to sync. Sign-in works without them:

1. Enable the Calendar API: [APIs & Services → Library → Google Calendar API](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com) → **Enable**. Until you do, Settings → Calendar shows "The Google Calendar API is off in your Google Cloud project".
2. On [Google Auth Platform → Data Access](https://console.cloud.google.com/auth/scopes), click **Add or remove scopes**. Add `https://www.googleapis.com/auth/calendar.readonly`, and add `https://www.googleapis.com/auth/calendar.events` only if you want Accept/Maybe/Decline from invite cards. Then click **Update → Save**. Both scopes are **sensitive**, not restricted.
3. The project is **In production (unverified)**. Adding sensitive scopes doesn't change that: the consent screen still says "Google hasn't verified this app", and the same unverified-user cap applies (section 4). Existing accounts keep working. Each one grants the calendar scopes on top of Gmail through incremental consent when you click Connect calendar, and the Gmail grant is untouched.

What happens when a step is missing. Penguin always asks with `include_granted_scopes=true` and reads what was granted from the token response's `scope` field, so:

| Situation | What you see |
| --- | --- |
| Calendar box unticked on Google's screen | The account signs in with mail only. No error; Settings → Calendar shows "Not connected" with Connect calendar. |
| Calendar API not enabled | Sign-in works. Settings → Calendar shows "The Google Calendar API is off…" for that account until you enable it. |
| `calendar.readonly` not on the Data Access page | Sign-in works. Google only uses that list for verification: a scope missing from it makes the app count as unverified, which this private app already is. Google documents this under [Unverified apps](https://support.google.com/cloud/answer/7454865?hl=en). List the scopes anyway so the consent configuration matches what Penguin asks for. |
| A Workspace admin allows Gmail but blocks Calendar (a **Specific Google data** policy without Calendar, or Calendar marked restricted) | Google refuses the **whole** sign-in with `admin_policy_enforced`. Untick **Also connect Google Calendar** and sign in again with mail only, or have the admin add Calendar (section 6). |

## 6. Workspace administrator approval, if needed

Do this separately in **each Workspace organization** whose account is blocked. The same External client ID can be approved in every organization; approval in the project’s organization does not approve another organization’s mailbox. A personal `gmail.com` account has no Workspace admin console.

1. Sign in to [Google Admin console](https://admin.google.com/) as that organization’s administrator.
2. Open **☰ → Security → Access and data control → API controls → Manage App Access**. This opens **App access control**.
3. Under **Configured apps**, click **Configure new app**.
4. Paste Penguin’s complete **OAuth Client ID** → **Search** → select the matching app.
5. Under **Scope**, select your organizational unit → **Continue**. Use the top organization only if everyone there should get this policy.
6. Under **Access to Google data**, select **Trusted → Continue → Finish**. If editing an existing entry, save its new access setting.

Trusted allows requested access to restricted services; user authorization still applies. A narrower **Specific Google data** policy must include Gmail modify and all three sign-in permissions, plus Calendar (read-only) unless you turn off **Connect calendar when adding accounts** (section 5c). **Limited** will not allow restricted Gmail access. Internal apps can also need approval. [Current app-access controls](https://knowledge.workspace.google.com/admin/apps/control-which-apps-access-google-workspace-data).

Other policy checks:

| Setting | What it means for Penguin |
| --- | --- |
| **Less secure apps / app passwords** | Irrelevant: Penguin uses OAuth and Gmail API. Workspace ended ordinary username/password access for these clients on May 1, 2025. [Google’s transition guidance](https://support.google.com/a/answer/9003945). |
| **Advanced Protection — Workspace** | Apps with high-risk scopes can be blocked unless explicitly trusted by the administrator. Use the client-specific approval above. [Workspace Advanced Protection](https://knowledge.workspace.google.com/admin/security/protect-users-with-the-advanced-protection-program). |
| **Advanced Protection — personal Gmail** | Unverified third-party access to Gmail may be blocked without a proceed link. Workspace trust settings cannot apply to this account; publishing alone does not make the private client eligible. [Advanced Protection FAQ](https://support.google.com/accounts/answer/7539956?hl=en). |
| **Google Session control** | **Security → Access and data control → Google Session control** controls web sessions. Google says these durations are not enforced on OAuth-authenticated apps. Do not shorten/lengthen web sessions to fix Penguin’s seven-day Testing token. [Session-length rules](https://knowledge.workspace.google.com/admin/security/set-session-length-for-google-services). |
| **Google Cloud session control / `invalid_rapt`** | Cloud reauthentication can invalidate tokens using the `cloud-platform` scope. Penguin’s four scopes do not include it, so this is not its normal Gmail token lifetime. It can affect the optional `gcloud` setup session. [Cloud session controls](https://knowledge.workspace.google.com/admin/security/set-session-length-for-google-cloud-services), [OAuth session-control errors](https://developers.google.com/identity/protocols/oauth2#expiration). |
| **Password changes, revocation, other token expiry** | Gmail-scoped tokens can stop working after a password change. Revocation, six months without token use, time-limited consent, or issuing more than 100 refresh tokens for one account/client can also require signing in again. Production is not a promise of permanent tokens. [Refresh-token expiration](https://developers.google.com/identity/protocols/oauth2#expiration). |

If you administer each organization, repeat the approval yourself under the appropriate admin login. Otherwise that organization’s administrator must approve the client. Changing OAuth project location does not override its access policy.

## 7. Connect your accounts — about 2 minutes

1. Open/restart Penguin so it reads the client JSON; choose **Add account**.
2. In the system browser, select the first Google account, or **Use another account**. Complete its normal sign-in/2-Step Verification.
3. If the unverified-app warning appears, expand **Advanced → Go to Penguin Private (unsafe)**; the displayed app identifier can vary before brand verification. Proceed only when this is the client you just created. [Google’s unverified-app explanation](https://support.google.com/cloud/answer/7454865?hl=en).
4. Review and grant the requested Gmail and identity access → **Continue**. Return to Penguin when the loopback callback finishes.
5. Repeat **Add account** for each other account, checking the selected address each time. Each account grants its own access; only one client JSON is needed.

Success means every address appears and newest mail begins syncing. Full-mailbox search fills in as backfill progresses. An unfinished app command is an implementation issue, not evidence that the Cloud configuration is wrong.

## Optional: use `gcloud` for project creation and API enablement

This replaces section 1 only. Use an [installed Google Cloud CLI](https://docs.cloud.google.com/sdk/docs/install) or Cloud Shell. Run `gcloud auth login` locally and select the intended project-owner account; Cloud Shell normally already has your console identity. [CLI authentication](https://docs.cloud.google.com/sdk/gcloud/reference/auth/login).

Choose a globally unique project ID; replace the example values before running:

```sh
gcloud auth login
gcloud organizations list
PENGUIN_PROJECT_ID='penguin-mail-unique123'
PENGUIN_ORG_ID='123456789012'
gcloud projects create "$PENGUIN_PROJECT_ID" \
  --name='Penguin Private' --organization="$PENGUIN_ORG_ID"
gcloud services enable gmail.googleapis.com --project="$PENGUIN_PROJECT_ID"
```

For a **No organization** project, authenticate as the personal account and replace the `projects create` command with:

```sh
gcloud projects create "$PENGUIN_PROJECT_ID" --name='Penguin Private'
```

Use only one creation variant. The organization ID comes from [`gcloud organizations list`](https://docs.cloud.google.com/sdk/gcloud/reference/organizations/list); omitting `--organization`/`--folder` creates without a parent. [Project command](https://docs.cloud.google.com/sdk/gcloud/reference/projects/create), [Gmail enablement command](https://developers.google.com/workspace/guides/enable-apis#gmail_api).

**Still use the consoles** for Branding, Audience, scopes, test users, publishing, Desktop OAuth client creation/download, and Workspace trust. Google requires manual OAuth-client configuration; `gcloud auth application-default login`, service accounts, and IAP client commands are not substitutes for Penguin’s Desktop credentials. [Manual client-creation requirement](https://developers.google.com/identity/protocols/oauth2/resources/best-practices#manual-creation-and-configuration-of-oauth-clients).

## Troubleshooting

| Symptom | Cause and action |
| --- | --- |
| `redirect_uri_mismatch` | Confirm the loaded JSON is for **Desktop app**, not Web application. Check the environment override. The OAuth request and token exchange must use the same actual loopback address/port/path. Fix the callback implementation if necessary; do not register the Vite UI port or edit JSON redirects to mask a wrong client type. [Desktop redirects](https://developers.google.com/identity/protocols/oauth2/native-app). |
| Browser cannot reach `127.0.0.1` after consent | Penguin’s listener must still be running on that Mac. Restart sign-in and check local firewall/proxy handling of loopback. A listener failure is distinct from Google rejecting the redirect URI. [Loopback requirements](https://developers.google.com/identity/protocols/oauth2/native-app#redirect-uri_loopback). |
| `access_blocked`, `access_denied`, or `org_internal` | Read the detailed reason. `org_internal`: use External for cross-organization accounts. Testing: add the exact sign-in account as a test user, or complete section 4. A policy block needs section 6, not another test-user entry. [Audience rules](https://support.google.com/cloud/answer/15549945?hl=en). |
| `invalid_grant` about seven days after connecting | Check that the **project belonging to the loaded client ID** is In production. Publish, then obtain a new token through sign-in. If it already is, investigate the other token-expiry causes in section 6. [Token expiration](https://developers.google.com/identity/protocols/oauth2#expiration). |
| `admin_policy_enforced` / Gmail `domainPolicy` | The mailbox’s organization blocks the app/service. Its administrator must approve Penguin’s client ID and scopes for the correct organizational unit, then sign in again. [OAuth policy errors](https://developers.google.com/identity/protocols/oauth2/native-app#authorization-errors-admin-policy-enforced), [Gmail domain policy](https://developers.google.com/workspace/gmail/api/guides/handle-errors#domainpolicy). |
| “Google hasn’t verified this app” | Expected for this personal External client; follow section 7. If there is no proceed option, inspect administrator/Advanced Protection restrictions or an exhausted user cap. Publishing does not remove this warning. [Unverified apps](https://support.google.com/cloud/answer/7454865?hl=en). |
| Gmail API disabled / `accessNotConfigured` | Enable **Gmail API** in the project that issued the JSON, then retry. [API enablement](https://developers.google.com/workspace/guides/enable-apis). |
| `403 userRateLimitExceeded`, `rateLimitExceeded`, or `429` | Inspect the returned reason/retry time. Backfill needs pacing, limited concurrent requests, and exponential backoff; restarting it repeatedly adds load. Bandwidth and sending limits can also produce 429s. See the numbers below. [Gmail error handling](https://developers.google.com/workspace/gmail/api/guides/handle-errors). |
| `403 dailyLimitExceeded` | Inspect **APIs & Services → Gmail API → Quotas & System Limits** in the correct project. Check a configured usage cap and its reset; raise an accidental custom cap if appropriate. This error is not proof that the published daily billing threshold is an adjustable hard quota. [Daily-limit errors](https://developers.google.com/workspace/gmail/api/guides/handle-errors#dailylimitexceeded). |

## Current quotas and backfill time

**The old 250 units/user/second and 5 units per `messages.get` are not the defaults for a project created today.** Google changed quotas and retrieval costs on May 1, 2026. Projects with API use during November 2025–April 2026 retain their previously configured quotas for now; check an existing project’s actual limits rather than assuming it retains every old method cost. [Google’s quota-change announcement](https://developers.google.com/workspace/release-notes#May_01_2026).

| New-project limit or method | Current value |
| --- | ---: |
| Per user, per project | **6,000 quota units/minute** (100/second averaged) |
| Across the project | **1,200,000 quota units/minute** |
| `messages.get` | **20 units/message** |
| `messages.list` | **5 units/request** |
| Daily project billing threshold | **80,000,000 units/24 hours** |

These are Google’s [current Gmail API quotas](https://developers.google.com/workspace/gmail/api/reference/quota). The daily number is a **billing threshold**, not a published per-user daily message-fetch allowance. Standard usage is currently without additional Gmail API charges; Google says billing above thresholds will follow at least 90 days’ notice. The checked docs do not supply a final price or effective billing date. [2026 billing rollout](https://developers.google.com/workspace/tools-safety).

The following are **calculated quota-only minimums**, not measured Penguin performance. Assume one `messages.get` per message, sustained full quota, and no listing, retries, attachments, network, bandwidth, or indexing overhead:

| Mailbox messages | Requested old-rate scenario: 250 ÷ 5 = 50 messages/s | New-project budget: 6,000 ÷ 20 = 300 messages/min |
| --- | ---: | ---: |
| 50,000 | **16 min 40 sec** | **2 hr 46 min 40 sec** |
| 250,000 | **1 hr 23 min 20 sec** | **13 hr 53 min 20 sec** |
| 1,000,000 | **5 hr 33 min 20 sec** | **55 hr 33 min 20 sec** |

Formula: `seconds = message_count × units_per_get ÷ units_per_second`. The old-rate column is the requested hypothetical comparison, not a promise for grandfathered projects. At an illustrative 70% backfill budget on a new project, allow about **4 hours / 20 hours / 80 hours**, before additional bottlenecks. Each account has its own user budget; three parallel accounts still share the project budget. Three users at the new per-user maximum consume at most **25.92 million units/day** combined, below the daily threshold.

Bandwidth can make those estimates substantially longer: Gmail API has per-user bandwidth limits equal to, but independent of, IMAP, shared across that user’s Gmail API clients. Workspace’s published IMAP limits are **2,500 MB downloaded/day** and **500 MB uploaded/day**. A mailbox’s total transferred bytes therefore matter as well as its message count. [API bandwidth behavior](https://developers.google.com/workspace/gmail/api/guides/handle-errors#bandwidth_limits), [bandwidth numbers](https://knowledge.workspace.google.com/admin/gmail/gmail-bandwidth-limits).

Sending is separately limited: normally **2,000 messages/user per rolling 24 hours** for paid Workspace, **500** for Workspace trial accounts; personal Gmail can block sending above **500 emails/day**. These are not backfill limits. [Workspace sending limits](https://knowledge.workspace.google.com/admin/gmail/gmail-sending-limits-in-google-workspace), [personal Gmail limits](https://support.google.com/mail/answer/22839?hl=en).

## Later, if Penguin becomes public

A broadly distributed client using `gmail.modify` needs Google’s brand and restricted-scope verification: a real homepage/privacy policy, verified domain ownership, scope justification, and a demonstration of the consent flow and features. Budget review time before launch. [Restricted-scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification).

**CASA is a separate security assessment.** Google’s stated trigger is the ability to access restricted user data from or through a third-party server, with recurring assessment at least annually. A wholly on-device Penguin has a basis for avoiding that server-assessment requirement, subject to Google’s review of the actual data flow; it still needs public OAuth verification. Adding a server that can access mail or usable refresh tokens changes that analysis. See [Google’s assessment criteria and CASA link](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification#security-assessment). None of this requires you to submit a qualifying private personal setup for verification now.
