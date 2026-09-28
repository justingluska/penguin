# Calendar invitations

How Penguin shows and answers meeting invitations, and why it works the way it does. Code: `crates/penguin-core/src/store_invites.rs` (storage, row chip), `crates/penguin-gmail/src/{ics,imip}.rs` (parse, answer), `crates/penguin-gmail/src/calendar.rs` (`respond_with`), `crates/penguin-graph/src/meeting.rs`, `apps/desktop/src-tauri/src/calendar/{mod,invites}.rs`, UI `apps/desktop/src/features/calendar/{InviteChip,InviteCard,invite,inviteState}.ts(x)`, mock `src/lib/mock/invites.ts`.

## What other clients do

- **Apple Mail / Calendar, Thunderbird, Fastmail, Spark**: they read the `text/calendar` part (iTIP, RFC 5546) and answer with an iMIP email (RFC 6047): a `METHOD:REPLY` object carrying your `PARTSTAT` for the event's `UID`/`SEQUENCE`/`RECURRENCE-ID`, sent to the `ORGANIZER`. Every calendar server applies an incoming REPLY to the organizer's copy (Google, Exchange, iCloud, Fastmail).
- **Gmail and Google Calendar**: when the event is on your Google Calendar, answering changes your attendee `responseStatus` (the API equivalent is `events.patch` of your attendee row with `sendUpdates=all`). Google then tells the organizer natively, and your own calendar stays in sync. There is no API for "propose a new time": Google's proposal UI exists only in Google Calendar itself.
- **Outlook / Microsoft 365**: invitations are `eventMessage` items linked to an event on your calendar. Graph's `accept`, `tentativelyAccept` and `decline` take a `comment`, `sendResponse`, and, for tentative and decline only, `proposedNewTime` (400 if the organizer turned proposals off). On the wire, Outlook proposals are iTIP `METHOD:COUNTER`.
- **Superhuman and Spark**: Yes / No / Maybe inline on the message and in the list, the event time and conflicts next to it.
- **Updates** are REQUESTs with a higher `SEQUENCE` (Google also bumps it for guest changes); clients show "Updated invitation" and many (Outlook, Fastmail) show what changed. **Cancellations** are `METHOD:CANCEL` (or `STATUS:CANCELLED`); the organizer's server removes the event from attendees' calendars that it manages.

## What Penguin does

Answers go out only on an explicit click. The route is chosen per account (`answer_route`):

| Account | Yes / Maybe / No and a note | Propose new time |
|---|---|---|
| Google, RSVP on (`calendar.events`), event on your calendar | Google Calendar API: `events.patch` of your attendee row (`responseStatus`, `comment`), `sendUpdates=all`. A whole series patches the master event. | iMIP `COUNTER` email to the organizer (the API can't propose), plus your answer in Google Calendar |
| Google without RSVP, or the event isn't on your calendar | iMIP `REPLY` email to the organizer | iMIP `COUNTER` email |
| Microsoft | Graph event action on the linked event, falling back to iMIP email when Graph can't (no calendar permission, no linked event) | Graph `proposedNewTime` (Maybe / No only), same fallback |
| IMAP | iMIP `REPLY` email | iMIP `COUNTER` email |

- The iMIP answer echoes exactly what identifies the event (UID, SEQUENCE, RECURRENCE-ID, DTSTART/DTEND or DURATION with their VTIMEZONEs, ORGANIZER) and names you as the invite addressed you. Everything Penguin writes is escaped (RFC 5545 §3.3.11) and folded at 75 octets. The email is `multipart/alternative` (a plain-text line, then `text/calendar; method=REPLY|COUNTER`), threaded under the invitation with In-Reply-To/References. A proposal's times go in UTC.
- Why not always email? For a Google account whose calendar Penguin can edit, the API keeps your own Google Calendar in step and the organizer gets Google's native response. Email is the route that works for every organizer (Google, Outlook, iCloud, Fastmail), so it is the fallback everywhere.
- Gmail without RSVP answers by email; the card says so once ("Penguin emails your answer to Priya") with Turn on RSVP (Settings → Calendar), and the first email answer's toast repeats the offer. Answering by email doesn't change your own Google Calendar copy until Google processes the organizer's update.
- Microsoft: Penguin doesn't ask for `Calendars.ReadWrite` yet, so today Graph answers fail with 403 and the email route is used. Outlook invitations that arrive without an `.ics` part (Exchange converts most of them to `eventMessage`s) get no card or chip yet; both need the Graph calendar scope and reading `eventMessage` fields at sync (future work).

## Showing invitations

- **Scanner** (`calendar::invites::spawn_scanner`, woken when local mail changes; no timer otherwise): messages from the last 45 days with a calendar part and no `invites` row yet (a partial index on `attachments.kind = 6`), 25 per round. Each part is fetched once through the attachment cache and parsed into an `InviteSnapshot` stored in `invites(account_id, message_id, …, data)`; '' = nothing usable (or your own REPLY), not retried. A full round (25) or one stopped offline or throttled runs again a minute later; a part that can't be had (gone, refused, unreadable) is logged and stored as '' so it can't stall the older ones behind it. Opening a thread (`event_invite`) stores its snapshot too.
- **Row chip** (`ThreadSummary.invite`, filled by `Store::attach_invites` in `list_threads`: a primary-key lookup per row with attachments, local only): the newest invitation in the thread, its time ("Tue Sep 29 · 10–11am"), Updated / Canceled, your answer (newest of what Penguin sent, the synced event, the invite's own PARTSTAT; a newer SEQUENCE asks again), and the first busy event it overlaps. Replies to your events show "Ana accepted".
- **Card**: above the messages in both thread layouts, the reading pane beside the list and the opened thread. The synced copy when it is on your calendar (a series shows its next instance), your time and the organizer's (their TZID, when it is an IANA zone and differs), map link, guests collapsed with counts, conflicts, Yes / Maybe / No, Add a note, Propose new time (a day and time picker; the next weekday by default), Open in Calendar (the Calendar surface on that day, event selected), "What changed" against the previous stored version of the same UID/instance (time, location, title, guests), and a red banner for cancellations.
- **Optimistic answers** (`inviteState.ts`): the click shows at once in the row and the card; the backend records it in `invite_responses` and emits mail-changed; a failure puts back what was there and shows the reason in a toast. The override holds until the refreshed data differs from what was on screen when you clicked.

## Verified vs needs a real account

Unit-tested: REPLY/COUNTER generation (escaping, folding, UID/SEQUENCE/DTSTAMP/RECURRENCE-ID/ATTENDEE PARTSTAT, time zones), the email around it, parsing updates and cancellations into snapshots and diffs, answer precedence, routes per provider, the Google patch (note, series), the Graph action against the fake Graph server, the chip and card logic (TypeScript). Fixtures of real Google Calendar mail (`crates/penguin-gmail/tests/fixtures/google_invite*.json`: an invitation and its "Updated invitation", text/calendar inline or out of line plus invite.ics) go through conversion, the store, the pick, parsing, the card and the chip. When a thread shows no card, penguin.log (Copy debug info) says why: `invite: no usable calendar part` (with each part's type and size), `calendar part download failed`, `didn't parse` or `card failed`. Not yet tried against real servers: how Google, Outlook and iCloud organizers display Penguin's REPLY and COUNTER emails, and Graph with a real `Calendars.ReadWrite` grant.
