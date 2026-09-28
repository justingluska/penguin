# Privacy features: tracking protection and read receipts

What Penguin does about email tracking, what each setting changes, and why. Research date 2026-09-27; sources at the end. The sanitizer internals are in `docs/SECURITY.md`.

## What senders can learn, and what Penguin blocks

| Tracking method | What it tells the sender | What Penguin does | Setting (Settings → Privacy) |
|---|---|---|---|
| Remote images (logos, photos) | That you opened it, when, your IP (rough location), device | Held back until you choose Load images | **Remote images**: Ask (default) / Always / Never, plus "Always load images from" verified senders |
| Tracking pixels: known trackers (`trackers.rs`, about 90 rules) and tiny (≤3×3) or hidden images | Same as above; the pixel exists only to report the open | Removed, even when images load | **Block tracking pixels**: on (default). Off: pixels are treated like any other remote image, and the privacy row warns when one loaded |
| Per-person link parameters (`mc_eid`, `_hsenc`, `mkt_tok`, `fbclid`, `gclid`, …) | Ties the visit to you when you open the link | Removed from links before you open them | **Remove tracking from links**: off (default) |
| Click-tracking redirects (`…/ls/click?upn=…`) | That you clicked, and which link | Counted and explained; can't be removed | none (only visiting the link would reveal where it goes) |

The defaults are what Penguin did before these settings existed: pixels blocked, images on Ask, links untouched.

### Per-message indicator

The row above each message body says what was done to it (`features/message-body/privacy.ts`, `privacySummary`):

- `Remote images blocked (3) · Load images · Always from this sender · 2 trackers removed · 3 links cleaned`
- With nothing held back, a quiet shield: `2 trackers removed · 3 links cleaned`.
- With pixel blocking off and images loaded, an amber eye: `2 trackers loaded`.

Clicking it opens the privacy details: each tracker by company (or host), path without the query string, how it was recognized, and its status (removed, waiting with images, loaded); the remote images; and the links section, which names the removed parameters (never their values) and counts links that go through a click tracker. Message details show the same counts.

### Why these link parameters, and not `utm_*`

`crates/penguin-render/src/links.rs` removes exactly Firefox's query-stripping list (the `query-stripping` Remote Settings collection, fetched 2026-09-27) plus the entries of Brave's `query-filter.json` that are per-person or per-click ids and not tied to one site (`ml_subscriber`, `sfmc_id`, `ss_email_id`, `bsft_clkid`, `igshid`, `ttclid`, …). Campaign tags (`utm_*`, `mc_cid`) stay: they name the newsletter, not the reader, and neither browser removes them. Apple's Link Tracking Protection works the same way. WebKit describes it as removing "a subset of query parameters that have been identified as being used for pervasive cross-site tracking granular to users or clicks."

Links to unsubscribe or preference pages keep every parameter, because some identify the reader with exactly these. Marketo's unsubscribe page, for example, reads `mkt_tok`. The Unsubscribe button always acts on the link as the sender wrote it.

### What other clients do

- **Gmail** proxies images through Google's servers (since 2013), so senders don't get your IP, but they still see the open. "Ask before displaying external images" is an opt-in setting.
- **Apple Mail Privacy Protection** downloads remote content privately in the background when a message arrives. The sender sees an "open" for every message whether you read it or not, and never sees your IP.
- **HEY** strips anything that looks like a spy pixel, names who put it there, and proxies images through its own servers.
- **Spark** blocks 1×1 tracking pixels by default and sells its own pixel-based read statuses.
- **Mimestream** has a "Prevent tracking when viewing messages" option that blocks common trackers "on a best-effort basis".
- **Superhuman** has an Image Settings choice, "Block all known tracking pixels".

Penguin can't proxy images, because it has no server and won't run one. Loading images therefore reveals your IP to their hosts. That is why images stay on Ask by default.

## Read statuses ("see when they opened it")

A requested feature (Superhuman has it) lets the sender see when a message was opened. Penguin has no server, and it must never add tracking to mail without the sender knowing. Within those rules there are two ways to do this. Penguin ships the second one.

### What Superhuman does, and what changed in 2019

- On 2019-07-02 Mike Davidson published "Superhuman is Spying on You". It showed that a Superhuman sender saw "a running log of every single time you have opened my email, including your location when you opened it", whatever mail app the recipient used.
- On 2019-07-03 Rahul Vohra replied in "Read Statuses": "We have stopped logging location information for new emails, effective immediately." "We are deleting all historical location data from our apps." "We are keeping the read status feature, but turning it off by default." He also added a setting that blocks tracking pixels while other images still load.
- Superhuman's help center today (article updated 2026-09-01):
  - Read Statuses are off until you turn them on (⌘K → Enable Read Statuses), and they are set per account.
  - They show "the time, device, and each person who opened the message", with a Recent Opens Feed on the Business and Enterprise plans.
  - Nothing tells the recipient.
  - Superhuman's deliverability article advises "Disable Read Statuses" for mail that lands in spam.

### Option 1, not built: a tracking pixel on a server you run yourself

Penguin would put a unique 1×1 image in each message you send. The image would point at a small Cloudflare Worker that you deploy to your own account from a template in the repo, and Penguin would poll that Worker for opens. No Penguin server is needed: the Workers free plan allows 100,000 requests a day, and Workers KV allows 1,000 writes a day, so about 1,000 recorded opens a day.

It isn't built, for six reasons:

1. **It's wrong much of the time.** Apple Mail Privacy Protection downloads remote content when a message arrives. Mailchimp says Apple Mail messages are reported as "opened," regardless of the contact's activity. Gmail fetches images through its own proxy. And every client that blocks pixels never reports an open: Penguin itself, HEY, Spark by default, Mimestream's tracker blocking, and Superhuman's own "Block all known tracking pixels". So "opened" often isn't true, and "not opened" often isn't either.
2. **Group mail.** One pixel can't say which recipient opened the message. Telling them apart needs a separate copy of the message per recipient, which changes threading and what Bcc means.
3. **Your own opens.** Opening your Sent copy in Gmail's web app triggers the pixel.
4. **Consent and law.** It tracks a person without their knowledge. The UK ICO says that if an email "includes tracking pixels, you must comply with the rules on storage and access technologies" (PECR). France's CNIL recommendation of 2026-04-14 (délibération 2026-042) says when email tracking pixels need consent, with only narrow exemptions. A user sending tracked personal mail to people in the EU or UK takes on that exposure. The log of recipients' IP addresses would be theirs to protect.
5. **It contradicts Penguin.** Penguin removes these pixels from the mail it receives (see above), so adding them to the mail it sends would be building the thing it blocks. Penguin's own hidden-image rule would also remove it for every Penguin recipient.
6. **Running it.** The user has to deploy the Worker, secure it (a token for polling), and keep it up. Anyone who learns the image URL can fake opens.

### Option 2, built: standard read receipts (RFC 8098)

A read receipt is a visible request that the recipient's mail app answers, usually by asking the recipient first.

- **Asking.** With Settings → Privacy → **Ask for read receipts** on (it's off by default), every message you send carries `Disposition-Notification-To: <your address>` (`penguin-provider` compose, `Draft.requestReadReceipt`). On Microsoft accounts Penguin sets Graph's `isReadReceiptRequested` instead. It's one switch for all mail, like Outlook's tracking option.
  - `send_message` and `save_draft` apply the setting when the composer leaves it unset, so a scheduled send, which sends the saved draft, keeps it.
  - Rule forwards and anything else Penguin sends by itself never ask.
- **The recipient decides.** RFC 8098: "The presence of a Disposition-Notification-To header field in a message is merely a request for an MDN. The recipients' user agents are always free to silently ignore such a request", and "the default value should be not to send MDNs."
  - Gmail supports receipts for "work or school accounts" only. Personal Gmail doesn't. A Workspace admin has to turn them on. When the admin allows receipts to any address, "Prompt the user for each read receipt request" is forced on, so recipients approve each one.
  - Outlook: "the message recipient can decline to send read receipts … There is no way to force a recipient to send a read receipt."
  - Apple Mail ignores the request, as far as community reports go. There is no Apple source for this.
- **Reading what comes back.** A receipt is an email with a `message/disposition-notification` part.
  - A background scanner (`src-tauri/src/receipts.rs`, the same pattern as the invitation scanner) downloads that small part once. It is parsed by `penguin_core::receipts::parse_mdn`, reading `Original-Message-ID`, `Final-Recipient` and `Disposition`, and stored in `receipts_mdn`. That is a feature-owned schema (`Store::migrate_receipts`), and a partial index over `attachments` finds receipt emails.
  - The sent message then shows **Read by Alex Rivera · 3:12 PM** (`features/receipts/`). Its tooltip says what a missing receipt means: nothing.
  - `get_thread` and `load_remote_images` fill `MessageView.readReceipts` for messages labeled SENT, matched on their Message-ID.
- **Mail you receive.** Penguin never sends read receipts for it. RFC 8098 allows ignoring the request, and it is the privacy-preserving default. An "ask me each time" option could come later, and would always be off by default.

What's left to check on a real account:

- **The Gmail API route.** Gmail's API documents no read-receipt field. Penguin relies on the header travelling in the raw MIME that `messages.send` / `drafts.send` receive, and that needs an end-to-end check with a Workspace recipient (this build box has no accounts).
- **A per-message toggle.** One in the composer belongs to the compose owner. Until then the setting's description says it applies to every message.

## Sources

Read statuses and receipts:

- Mike Davidson, "Superhuman is Spying on You" (2019-07-02): https://mikeindustries.com/blog/archive/2019/06/superhuman-is-spying-on-you
- Rahul Vohra, "Read Statuses" (2019-07-03): https://blog.superhuman.com/read-statuses/
- Superhuman, Keeping Your Emails Out of Spam (updated 2026-06-12): https://help.superhuman.com/hc/en-us/articles/46005520093453-Keeping-Your-Emails-Out-of-Spam
- Superhuman, Pricing Plans (updated 2026-08-18): https://help.superhuman.com/hc/en-us/articles/46005733349517-Pricing-Plans
- RFC 8098, Message Disposition Notification: https://www.rfc-editor.org/rfc/rfc8098
- Gmail Help, read receipts: https://support.google.com/mail/answer/9413651
- Google Workspace Admin, let users request read receipts: https://support.google.com/a/answer/1383374 (it redirects to knowledge.workspace.google.com)
- Microsoft, read receipts in Outlook: https://support.microsoft.com/en-us/outlook/mail/add-and-request-read-receipts-and-delivery-notifications-in-outlook
- Mailchimp, Apple Mail Privacy Protection FAQ: https://mailchimp.com/help/apple-privacy-faq/
- Cloudflare Workers limits: https://developers.cloudflare.com/workers/platform/limits/ (KV: https://developers.cloudflare.com/kv/platform/limits/)
- UK ICO, direct marketing by email, tracking pixels: https://ico.org.uk/for-organisations/direct-marketing-and-privacy-and-electronic-communications/guidance-on-direct-marketing-using-electronic-mail/what-else-do-we-need-to-consider/
- CNIL, recommandation pixels de suivi dans les courriels (2026-04-14): https://www.cnil.fr/fr/recommandation-pixel-suivi-courriels

Tracking protection:

- Gmail blog, "Images now showing" (2013-12-12): https://gmail.googleblog.com/2013/12/images-now-showing.html
- Gmail Help, external images: https://support.google.com/mail/answer/145919
- Apple Mail Privacy Protection: https://support.apple.com/guide/mail/protect-email-privacy-mlhlp1205/mac
- Apple newsroom, Link Tracking Protection (2023-06-05): https://www.apple.com/newsroom/2023/06/apple-announces-powerful-new-privacy-and-security-features/
- WebKit, "Private Browsing 2.0" (2024-07-16): https://webkit.org/blog/15697/private-browsing-2-0/
- Firefox query stripping: https://firefox-source-docs.mozilla.org/toolkit/components/antitracking/anti-tracking/query-stripping/index.html. The live list: https://firefox.settings.services.mozilla.com/v1/buckets/main/collections/query-stripping/records
- Brave query filter list: https://github.com/brave/adblock-lists/blob/master/brave-lists/query-filter.json
- HEY spy trackers: https://www.hey.com/spy-trackers/
- Spark read statuses and pixel blocking: https://sparkmailapp.com/help/spark-for-teams/read-statuses
- Mimestream viewing settings: https://mimestream.com/help/user-guide/viewing-settings
- Superhuman, Read Statuses and Recent Opens Feed (updated 2026-09-01): https://help.superhuman.com/hc/en-us/articles/46005603745293-Read-Statuses-and-Recent-Opens-Feed
