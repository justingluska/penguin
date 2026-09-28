//! Google Calendar in Penguin: the Calendar view, invitation cards in
//! threads, "last/next meeting with X", events in search and the status
//! bar's "Next up". OWNER: calendar agent.
//!
//! - Connected per account: adding or reconnecting an account asks for
//!   `calendar.readonly` in the same Google consent (setting
//!   `calendar.connectOnSignIn`, default on; see [`sign_in_scopes`]), and
//!   Settings → Calendar → Connect asks for it later through incremental
//!   auth. RSVP is a second opt-in for `calendar.events`.
//! - Sync (penguin-gmail `calendar`) runs every ~5 minutes and on window
//!   focus (at most once a minute), then emits `penguin://calendar-changed`.
//! - Every read command answers from the local store; only `event_invite`
//!   may fetch (the invite's .ics part, once, then cached on disk), and
//!   `respond_to_event` writes to Google.

pub mod commands;
pub mod invites;

use penguin_core::{
    AccountProvider, AttachmentMeta, CalendarEvent, CalendarInfo, InviteResponse, InviteSnapshot,
    Message, Store,
};
use penguin_gmail::calendar::{CALENDAR_EVENTS_SCOPE, CALENDAR_READONLY_SCOPE};
use penguin_gmail::ics::{Invite, MAX_ICS_BYTES};
use serde::Serialize;

pub const EVENT_CALENDAR_CHANGED: &str = "penguin://calendar-changed";

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarChanged {
    pub account_ids: Vec<String>,
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
    /// Google account (Settings → Calendar offers it).
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
    messages.iter().rev().find_map(|m| {
        let parts: Vec<&AttachmentMeta> = m
            .attachments
            .iter()
            .filter(|a| is_calendar_part(a))
            .collect();
        let best = parts
            .iter()
            .find(|a| a.mime_type.eq_ignore_ascii_case("text/calendar"))
            .or_else(|| parts.first())?;
        Some((m, *best))
    })
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
        rsvp_available: route == "email" && ctx.google_calendar && !ctx.rsvp_granted,
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

#[cfg(test)]
mod tests;
