//! Invitations and calendar access, shared by the Mac (src-tauri
//! `calendar/`) and the iPhone (penguin-ffi): the invite card a thread shows
//! (when and where, conflicts, what changed, your answer and how it goes
//! out), picking and parsing the calendar part of a message, which calendar
//! scopes a sign-in asks for, and checking and recording an answer. Pure
//! over the local store and a parsed [`Invite`]: no network here; the apps
//! fetch the part's bytes and send the answer. Decisions: docs/INVITES.md.

use penguin_core::{
    AccountProvider, Address, AttachmentMeta, CalendarEvent, CalendarInfo, InviteResponse,
    InviteSnapshot, Message, Store,
};
use serde::Serialize;

use crate::calendar::{self as gcal, CALENDAR_EVENTS_SCOPE, CALENDAR_READONLY_SCOPE};
use crate::ics::{self, Invite, MAX_ICS_BYTES};
use crate::imip;

/// Calendar scopes an add-account or reconnect sign-in asks for, on top of
/// Gmail. `connect_on_sign_in` is the `calendar.connectOnSignIn` setting;
/// `granted` is what the account's current token already has (empty for a
/// new account). Read-only access when the setting is on, and on reconnect
/// whatever calendar access the account already had, so a sign-in after a
/// revoked token restores it instead of silently dropping RSVP. It never adds
/// `calendar.events` on its own: RSVP stays a separate opt-in.
///
/// Every scope here is optional: Google shows a checkbox per scope, and the
/// sign-in succeeds with Gmail alone (see [`calendar_granted`]).
pub fn sign_in_scopes(connect_on_sign_in: bool, granted: &[String]) -> Vec<&'static str> {
    let had = |scope: &str| granted.iter().any(|g| g == scope);
    let mut scopes = Vec::new();
    if connect_on_sign_in || had(CALENDAR_READONLY_SCOPE) {
        scopes.push(CALENDAR_READONLY_SCOPE);
    }
    if had(CALENDAR_EVENTS_SCOPE) {
        scopes.push(CALENDAR_EVENTS_SCOPE);
    }
    scopes
}

/// After a sign-in that asked for `requested`: whether the calendar is now
/// connected, so its first sync should start right away. False when nothing
/// calendar-related was asked for, or when the user unticked the calendar
/// box on Google's screen (the token's `scope` then lacks it): mail works,
/// and Settings → Calendar keeps offering Connect.
pub fn calendar_granted(requested: &[&str], granted: &[String]) -> bool {
    requested.contains(&CALENDAR_READONLY_SCOPE)
        && granted.iter().any(|g| g == CALENDAR_READONLY_SCOPE)
}

/// Settings → Calendar, per account.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAccountStatus {
    pub account_id: String,
    /// `calendar.readonly` granted: events sync.
    pub granted: bool,
    /// `calendar.events` granted too: RSVP from invite cards.
    pub rsvp_granted: bool,
    pub syncing: bool,
    /// Unix ms of the oldest successful sync among selected calendars.
    pub synced_at: Option<i64>,
    /// Events stored (selected calendars).
    pub events: u64,
    pub error: Option<String>,
    pub calendars: Vec<CalendarInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarStatus {
    pub accounts: Vec<CalendarAccountStatus>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventThread {
    pub account_id: String,
    pub thread_id: String,
    pub subject: String,
}

/// The event popover: the event, its calendar and the invitation thread.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDetail {
    pub event: CalendarEvent,
    pub calendar: Option<CalendarInfo>,
    pub thread: Option<EventThread>,
}

/// The card at the top of a thread that carries an invitation.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InviteCard {
    pub account_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub uid: Option<String>,
    /// request | cancel | reply | publish | counter | other
    pub method: String,
    /// The synced copy when the event is on one of your calendars (then
    /// `in_calendar`), else the invite's own details (empty calendar/id).
    pub event: CalendarEvent,
    pub in_calendar: bool,
    /// The invite is for a whole recurring series.
    pub recurring: bool,
    /// Busy events overlapping it on any of your calendars.
    pub conflicts: Vec<CalendarEvent>,
    /// Yes / Maybe / No work: a request you're invited to, not over, with a
    /// way to send the answer (`route`).
    pub can_respond: bool,
    /// How an answer goes out: calendar (Google Calendar API: the calendar
    /// and the organizer update) | graph (Microsoft Graph event action,
    /// falling back to email) | email (an iMIP REPLY to the organizer) |
    /// none.
    pub route: String,
    /// The answer goes by email only because RSVP isn't turned on for this
    /// Google account, whose calendar is connected (Settings → Calendar
    /// offers it). False while the calendar isn't connected at all.
    pub rsvp_available: bool,
    /// "Propose new time" works (not for a whole series). Google Calendar
    /// has no API for it, so it goes as an iMIP COUNTER email; Microsoft
    /// uses proposedNewTime.
    pub can_propose: bool,
    /// Your answer: newest of what Penguin sent, the synced event and the
    /// invite's own PARTSTAT. None when you aren't a guest.
    pub response: Option<String>,
    /// What Penguin last sent for this event (note, proposal, how).
    pub sent: Option<InviteResponse>,
    /// The organizer's time zone (DTSTART's TZID) when not UTC.
    pub time_zone: Option<String>,
    /// A request that changes an earlier version.
    pub updated: bool,
    /// time | location | title | guests that differ from `previous`.
    pub changes: Vec<String>,
    /// The version you had before this one, when it is stored.
    pub previous: Option<InviteSnapshot>,
    /// This account has calendar access (else the UI offers to connect).
    pub calendar_connected: bool,
}

/// Where an invite card comes from and what the account can do.
#[derive(Debug, Clone)]
pub struct CardContext<'a> {
    pub account_id: &'a str,
    pub thread_id: &'a str,
    pub message_id: &'a str,
    /// The invite message's date (orders versions for "What changed").
    pub message_date: i64,
    pub provider: AccountProvider,
    /// A Google account (Google Calendar can exist).
    pub google_calendar: bool,
    /// calendar.readonly granted.
    pub calendar_connected: bool,
    /// calendar.events granted (RSVP opt-in).
    pub rsvp_granted: bool,
}

/// How Yes / Maybe / No will be sent (see [`InviteCard::route`]).
pub fn answer_route(ctx: &CardContext, answerable: bool, synced_guest: bool) -> &'static str {
    if !answerable {
        "none"
    } else if ctx.google_calendar && ctx.rsvp_granted && synced_guest {
        "calendar"
    } else if ctx.provider == AccountProvider::Microsoft {
        "graph"
    } else {
        "email"
    }
}

pub fn is_calendar_part(a: &AttachmentMeta) -> bool {
    let mime = a.mime_type.to_ascii_lowercase();
    (mime == "text/calendar"
        || mime == "application/ics"
        || a.filename.to_ascii_lowercase().ends_with(".ics"))
        && (a.size as usize) <= MAX_ICS_BYTES
}

/// The newest message in the thread with a calendar part, and that part.
/// A `text/calendar` alternative (what Google/Outlook mark METHOD on) wins
/// over an `.ics` file on the same message.
pub fn pick_invite_part(messages: &[Message]) -> Option<(&Message, &AttachmentMeta)> {
    messages
        .iter()
        .rev()
        .find_map(|m| calendar_part(&m.attachments).map(|p| (m, p)))
}

/// Build the card from a parsed invite and the local store. Blocking.
pub fn invite_card(
    store: &Store,
    ctx: &CardContext,
    invite: &Invite,
    now_ms: i64,
) -> penguin_core::Result<InviteCard> {
    let snap = invite.to_snapshot();
    let account_id = ctx.account_id;
    let stored = store.synced_copy(account_id, &snap, now_ms)?;
    let in_calendar = stored.is_some();
    let event = stored
        .clone()
        .unwrap_or_else(|| invite.to_event(account_id));
    let method = invite.method_name().to_string();
    let over = event.end < now_ms && !penguin_core::store::is_series(&snap);
    let conflicts = if method == "cancel" || over || event.all_day {
        Vec::new()
    } else {
        store.event_conflicts(event.start, event.end, invite.uid.as_deref(), None)?
    };
    let synced_guest = stored
        .as_ref()
        .is_some_and(|e| e.attendees.iter().any(|a| a.is_self && !a.organizer));
    let answerable = penguin_core::store::answerable(&snap, now_ms)
        && !(in_calendar && event.end < now_ms && !penguin_core::store::is_series(&snap));
    let route = answer_route(ctx, answerable, synced_guest);
    let sent = match invite.uid.as_deref() {
        Some(uid) => store.invite_response(account_id, uid, invite.recurrence_id)?,
        None => None,
    };
    let previous = if method == "request" {
        store.previous_invite(account_id, &snap, ctx.message_id, ctx.message_date)?
    } else {
        None
    };
    let changes = previous
        .as_ref()
        .map(|p| snap.changes_since(p))
        .unwrap_or_default();
    Ok(InviteCard {
        account_id: account_id.to_string(),
        thread_id: ctx.thread_id.to_string(),
        message_id: ctx.message_id.to_string(),
        uid: invite.uid.clone(),
        can_respond: route != "none",
        // RSVP is offered only on top of a connected calendar: an account
        // without one is asked to connect its calendar (once, in Settings
        // and the agenda), never nudged about RSVP on every invite.
        rsvp_available: route == "email"
            && ctx.google_calendar
            && ctx.calendar_connected
            && !ctx.rsvp_granted,
        can_propose: route != "none" && !penguin_core::store::is_series(&snap),
        route: route.to_string(),
        response: if matches!(method.as_str(), "request" | "publish") {
            penguin_core::store::effective_response(&snap, stored.as_ref(), sent.as_ref())
        } else {
            None
        },
        sent,
        time_zone: snap.time_zone.clone(),
        updated: method == "request" && (snap.sequence > 0 || previous.is_some()),
        changes,
        previous,
        method,
        event,
        in_calendar,
        recurring: invite.recurring,
        conflicts,
        calendar_connected: ctx.calendar_connected,
    })
}

/// Settings → Calendar status for one account (without the live `syncing`
/// and `error` fields). Blocking.
pub fn account_status(
    store: &Store,
    account_id: &str,
    granted: bool,
    rsvp_granted: bool,
) -> penguin_core::Result<CalendarAccountStatus> {
    let counts = store.calendar_counts(account_id)?;
    Ok(CalendarAccountStatus {
        account_id: account_id.to_string(),
        granted,
        rsvp_granted,
        syncing: false,
        synced_at: counts.synced_at,
        events: counts.events,
        error: None,
        calendars: store.list_calendars(Some(&[account_id.to_string()]))?,
    })
}

/// The calendar part to read among a message's parts: a `text/calendar`
/// alternative (what Google and Outlook mark METHOD on) over an `.ics` file.
pub fn calendar_part(parts: &[AttachmentMeta]) -> Option<&AttachmentMeta> {
    parts
        .iter()
        .filter(|p| is_calendar_part(p))
        .find(|p| p.mime_type.eq_ignore_ascii_case("text/calendar"))
        .or_else(|| parts.iter().find(|p| is_calendar_part(p)))
}

/// Parse a downloaded calendar part (read as UTF-8, lossily, up to
/// [`MAX_ICS_BYTES`]) for the account at `account_email`.
pub fn parse_part(bytes: &[u8], account_email: &str) -> Option<Invite> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_ICS_BYTES)]);
    ics::parse_invite(&text, account_email)
}

/// What a failed calendar sync says to the user.
pub fn friendly_sync_error(e: &crate::Error) -> String {
    use crate::Error as E;
    if gcal::is_api_disabled(e) {
        "The Google Calendar API is off in your Google Cloud project. Enable it, then try again"
            .into()
    } else if gcal::is_not_granted(e) {
        "Google didn't allow calendar access. Connect the calendar again".into()
    } else {
        match e {
            E::RateLimited => "Google Calendar asked Penguin to slow down; retrying later".into(),
            E::NeedsReauth(_) => "Sign in to this account again to keep syncing".into(),
            E::Network(_) => "Offline; Penguin will retry".into(),
            other => other.to_string(),
        }
    }
}

/// A checked answer: what the user chose, the note trimmed (at most 2000
/// characters, None when empty) and the proposed time.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedAnswer {
    pub response: String,
    pub comment: Option<String>,
    pub proposal: Option<(i64, i64)>,
}

/// Check Yes / Maybe / No (`accepted` | `tentative` | `declined`), the
/// note and a proposed time before anything is loaded or sent.
pub fn check_answer(
    response: &str,
    comment: Option<String>,
    proposal: Option<(i64, i64)>,
) -> Result<CheckedAnswer, &'static str> {
    if !gcal::valid_response(response) {
        return Err("response must be accepted, tentative or declined");
    }
    let comment = comment
        .map(|c| c.trim().chars().take(2000).collect::<String>())
        .filter(|c| !c.is_empty());
    if let Some((start, end)) = proposal {
        if end <= start {
            return Err("the proposed time must end after it starts");
        }
    }
    Ok(CheckedAnswer {
        response: response.to_string(),
        comment,
        proposal,
    })
}

/// Whether the card as it is now takes this answer; the reason, worded for
/// the user, when it doesn't.
pub fn can_answer(card: &InviteCard, answer: &CheckedAnswer) -> Result<(), &'static str> {
    if !card.can_respond {
        return Err(if card.method == "cancel" {
            "This event was cancelled"
        } else {
            "This invitation can't be answered (it's over, or you organize it)"
        });
    }
    if answer.proposal.is_some() && !card.can_propose {
        return Err("A new time can't be proposed for a whole recurring series");
    }
    Ok(())
}

/// The iMIP REPLY / COUNTER to the organizer, from `me`, threaded under the
/// invitation message: the raw RFC 822 bytes to send.
pub fn email_answer(
    me: &Address,
    message: &Message,
    invite: &Invite,
    answer: &CheckedAnswer,
    now_ms: i64,
) -> Result<Vec<u8>, String> {
    let organizer = invite
        .organizer
        .clone()
        .ok_or_else(|| "this invitation has no organizer to answer".to_string())?;
    let imip_answer = imip::Answer {
        response: answer.response.clone(),
        comment: answer.comment.clone(),
        proposal: answer.proposal,
        attendee: me.clone(),
        dtstamp_ms: now_ms,
    };
    imip::answer_message(
        me,
        &organizer,
        invite,
        &imip_answer,
        message.message_id_header.as_deref(),
        &message.references,
    )
}

/// Answering through Google Calendar: a whole recurring series (the invite
/// names the series, not one instance, and the synced copy is an instance
/// of it) is answered on the series.
pub fn answers_series(card: &InviteCard, invite: &Invite) -> bool {
    card.recurring && invite.recurrence_id.is_none() && card.event.recurring_event_id.is_some()
}

/// What Penguin remembers it sent (`Store::record_invite_response`).
pub fn response_record(
    invite: &Invite,
    answer: &CheckedAnswer,
    via: &str,
    now_ms: i64,
) -> InviteResponse {
    InviteResponse {
        response: answer.response.clone(),
        via: via.to_string(),
        sequence: invite.sequence,
        comment: answer.comment.clone(),
        proposed_start: answer.proposal.map(|p| p.0),
        proposed_end: answer.proposal.map(|p| p.1),
        at: now_ms,
    }
}

/// Remember the invite's snapshot for the row chip (an answer you sent
/// yourself is remembered as none), then build its card. Blocking.
pub fn remember_and_card(
    store: &Store,
    ctx: &CardContext,
    message: &Message,
    invite: &Invite,
    now_ms: i64,
) -> penguin_core::Result<InviteCard> {
    let snap = (!invite.is_own_answer()).then(|| invite.to_snapshot());
    store.put_invite(
        ctx.account_id,
        &message.id,
        &message.thread_id,
        message.date,
        snap.as_ref(),
    )?;
    invite_card(store, ctx, invite, now_ms)
}

#[cfg(test)]
#[path = "invites_tests.rs"]
mod tests;
