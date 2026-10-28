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

use serde::Serialize;

/// The card, its context, scopes and answer checks are shared with the
/// iPhone (penguin-gmail `invites`).
pub use penguin_gmail::invites::*;

pub const EVENT_CALENDAR_CHANGED: &str = "penguin://calendar-changed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarChanged {
    pub account_ids: Vec<String>,
}
