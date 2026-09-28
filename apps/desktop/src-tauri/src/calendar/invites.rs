//! Invitations in mail: the scanner that parses calendar parts in the
//! background so list rows can show an event chip, the invite card, and
//! answering (Yes / Maybe / No, a note, a proposed new time) through the
//! best route for the account. OWNER: calendar agent. Decisions:
//! docs/INVITES.md.
//!
//! Routes (`answer_route`):
//! - Google account with RSVP (calendar.events) and the event on its
//!   calendar: Google Calendar API (events.patch of your attendee row,
//!   `sendUpdates=all`), so the calendar and the organizer both update.
//!   Google's API can't propose a time, so a proposal goes as an iMIP
//!   COUNTER email in addition.
//! - Microsoft: the Graph event action on the linked event (accept /
//!   tentativelyAccept / decline, proposedNewTime); when Graph can't (no
//!   calendar permission, no linked event) the answer goes by email.
//! - Everything else (IMAP, Google without RSVP or not on the calendar):
//!   an iMIP REPLY / COUNTER email to the organizer (penguin-gmail `imip`).
//!
//! Nothing is ever sent without an explicit click in the UI.

use std::sync::Arc;
use std::time::Duration;

use penguin_core::{InviteResponse, Message};
use penguin_gmail::calendar::{
    self as gcal, CalendarClient, CALENDAR_EVENTS_SCOPE, CALENDAR_READONLY_SCOPE,
};
use penguin_gmail::ics::{self, Invite};
use penguin_gmail::imip;
use penguin_provider::{Error as ProviderError, InvitationAnswer};
use serde::Deserialize;
use tauri::{AppHandle, State};

use super::commands::{emit_changed, granted_scopes, has_scope, Calendar};
use super::{invite_card, is_calendar_part, pick_invite_part, CardContext, InviteCard};
use crate::error::{CmdError, CmdResult, ErrorCode};
use crate::ops::{self, now_ms};
use crate::state::{blocking, AppState};

/// How far back the scanner looks for invitations.
const SCAN_DAYS: i64 = 45;
/// Calendar parts parsed per round (each may be one small download).
const SCAN_BATCH: usize = 25;
/// Pause before the next round when this one left work (a full batch, or
/// paused offline), so downloads stay paced.
const SCAN_EVERY: Duration = Duration::from_secs(60);
/// After new mail wakes the scanner, let a burst of sync commits land.
const SETTLE: Duration = Duration::from_secs(2);

/// A thread's invitation, parsed.
pub struct Loaded {
    pub account: penguin_core::Account,
    pub message: Message,
    pub invite: Invite,
}

/// The invitation in `thread_id` (the message `message_id`, else the
/// newest one with a calendar part), fetched once then from the cache.
pub async fn load_invite(
    state: &AppState,
    account_id: &str,
    thread_id: &str,
    message_id: Option<&str>,
) -> CmdResult<Option<Loaded>> {
    let store = state.store.clone();
    let (a, t) = (account_id.to_string(), thread_id.to_string());
    let Some(thread) = blocking(move || Ok(store.get_thread(&a, &t)?)).await? else {
        tracing::warn!(account = %account_id, thread = %thread_id, "invite: thread not in the local store");
        return Ok(None);
    };
    let picked = match message_id {
        Some(id) => thread
            .messages
            .iter()
            .find(|m| m.id == id)
            .and_then(|m| pick_invite_part(std::slice::from_ref(m))),
        None => pick_invite_part(&thread.messages),
    };
    let Some((message, part)) = picked else {
        // The UI asks only when it sees a calendar part, so say why none
        // qualified (e.g. over the size cap). Types and sizes only.
        let parts: Vec<String> = thread
            .messages
            .iter()
            .flat_map(|m| &m.attachments)
            .map(|a| format!("{}:{}", a.mime_type, a.size))
            .collect();
        tracing::warn!(account = %account_id, thread = %thread_id, messages = thread.messages.len(), parts = ?parts, "invite: no usable calendar part in the thread");
        return Ok(None);
    };
    let (message, part) = (message.clone(), part.clone());
    let account = state.account(account_id).await?;
    let provider = state.provider(account_id).await?;
    let bytes = crate::attachments::bytes(
        &state.paths,
        provider.as_ref(),
        account_id,
        &message.id,
        &part,
    )
    .await
    .inspect_err(|e| {
        tracing::warn!(account = %account_id, message = %message.id, part = %part.id, mime = %part.mime_type, code = ?e.code, error = %e.message, "invite: calendar part download failed");
    })?;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(ics::MAX_ICS_BYTES)]).into_owned();
    let Some(invite) = ics::parse_invite(&text, &account.email) else {
        tracing::warn!(account = %account_id, message = %message.id, part = %part.id, mime = %part.mime_type, bytes = bytes.len(), vevent = text.contains("BEGIN:VEVENT"), "invite: calendar part didn't parse");
        return Ok(None);
    };
    Ok(Some(Loaded {
        account,
        message,
        invite,
    }))
}

/// Build the card for a loaded invitation, remembering its snapshot for
/// the row chip.
pub async fn card(state: &AppState, loaded: &Loaded) -> CmdResult<InviteCard> {
    let granted = granted_scopes(state, &loaded.account.email).await;
    let (read, rsvp) = (
        has_scope(&granted, CALENDAR_READONLY_SCOPE),
        has_scope(&granted, CALENDAR_EVENTS_SCOPE),
    );
    let store = state.store.clone();
    let account = loaded.account.clone();
    let message = loaded.message.clone();
    let invite = loaded.invite.clone();
    blocking(move || {
        let snap = (!invite.is_own_answer()).then(|| invite.to_snapshot());
        store.put_invite(
            &account.id,
            &message.id,
            &message.thread_id,
            message.date,
            snap.as_ref(),
        )?;
        let ctx = CardContext {
            account_id: &account.id,
            thread_id: &message.thread_id,
            message_id: &message.id,
            message_date: message.date,
            provider: account.provider,
            google_calendar: account.capabilities.calendar,
            calendar_connected: read,
            rsvp_granted: rsvp,
        };
        Ok(invite_card(&store, &ctx, &invite, now_ms())?)
    })
    .await
}

// ---------- the scanner ----------

/// Parse new calendar parts in the background, so rows show their event
/// chip without the thread being opened first. Calendar parts arrive only
/// with mail, so between rounds it sleeps until local mail changes
/// (`AppState::mail_changes`) instead of polling every minute; a round that
/// leaves work behind comes back after `SCAN_EVERY`.
pub fn spawn_scanner(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        // Subscribed before the startup pause: changes during it are seen.
        let mut changes = state.mail_changes();
        // Let the startup sync go first.
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            // This round covers every change so far (its own mail-changed
            // events wake one more, empty, round).
            changes.borrow_and_update();
            if scan_round(&state).await == Round::More {
                tokio::time::sleep(SCAN_EVERY).await;
                continue;
            }
            if changes.changed().await.is_err() {
                return;
            }
            tokio::time::sleep(SETTLE).await;
        }
    });
}

#[derive(PartialEq, Eq)]
enum Round {
    /// Nothing left that another round could do now.
    Done,
    /// A full batch, a read failure or a pause offline: try again soon.
    More,
}

async fn scan_round(state: &Arc<AppState>) -> Round {
    let store = state.store.clone();
    let since = now_ms() - SCAN_DAYS * 24 * 3600 * 1000;
    let Ok(candidates) =
        blocking(move || Ok(store.invite_scan_candidates(since, SCAN_BATCH)?)).await
    else {
        return Round::More;
    };
    let mut round = if candidates.len() >= SCAN_BATCH {
        Round::More
    } else {
        Round::Done
    };
    let mut touched: std::collections::HashMap<String, Vec<String>> = Default::default();
    for c in candidates {
        let Ok(account) = state.account(&c.account_id).await else {
            continue;
        };
        let Ok(provider) = state.provider(&c.account_id).await else {
            continue;
        };
        let part = c
            .parts
            .iter()
            .filter(|p| is_calendar_part(p))
            .find(|p| p.mime_type.eq_ignore_ascii_case("text/calendar"))
            .or_else(|| c.parts.iter().find(|p| is_calendar_part(p)));
        let snapshot = match part {
            None => None,
            Some(part) => {
                match crate::attachments::bytes(
                    &state.paths,
                    provider.as_ref(),
                    &c.account_id,
                    &c.message_id,
                    part,
                )
                .await
                {
                    Ok(bytes) => {
                        let text =
                            String::from_utf8_lossy(&bytes[..bytes.len().min(ics::MAX_ICS_BYTES)])
                                .into_owned();
                        let parsed = ics::parse_invite(&text, &account.email);
                        if parsed.is_none() {
                            tracing::warn!(account = %c.account_id, message = %c.message_id, mime = %part.mime_type, bytes = bytes.len(), vevent = text.contains("BEGIN:VEVENT"), "invite scan: calendar part didn't parse");
                        }
                        parsed
                            .filter(|i| !i.is_own_answer())
                            .map(|i| i.to_snapshot())
                    }
                    // Offline or throttled: try again next round.
                    Err(e) if e.code == ErrorCode::Network => {
                        tracing::debug!(account = %c.account_id, error = %e.message, "invite scan paused");
                        round = Round::More;
                        break;
                    }
                    // Signed out: this account's parts wait for the sign-in.
                    Err(e) if e.code == ErrorCode::NeedsReauth => continue,
                    // Gone on the server, refused (400/403), unreadable:
                    // nothing to show, and don't retry. Pausing the round
                    // here instead would stall every older invitation
                    // behind this one for good (opening the thread still
                    // tries again).
                    Err(e) => {
                        tracing::warn!(account = %c.account_id, message = %c.message_id, mime = %part.mime_type, code = ?e.code, error = %e.message, "invite scan: calendar part unavailable");
                        None
                    }
                }
            }
        };
        let store = state.store.clone();
        let has = snapshot.is_some();
        let c2 = c.clone();
        let stored = blocking(move || {
            Ok(store.put_invite(
                &c2.account_id,
                &c2.message_id,
                &c2.thread_id,
                c2.date,
                snapshot.as_ref(),
            )?)
        })
        .await;
        if stored.is_ok() && has {
            touched.entry(c.account_id).or_default().push(c.thread_id);
        }
    }
    for (account, threads) in touched {
        state.emit_mail_changed(&account, threads);
    }
    round
}

// ---------- answering ----------

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub start: i64,
    pub end: i64,
}

/// Send an iMIP REPLY / COUNTER to the organizer from the account,
/// threaded under the invitation.
async fn send_email_answer(
    state: &AppState,
    loaded: &Loaded,
    answer: &imip::Answer,
) -> CmdResult<()> {
    let organizer = loaded
        .invite
        .organizer
        .clone()
        .ok_or_else(|| CmdError::invalid("this invitation has no organizer to answer"))?;
    let (in_reply_to, references) = ops::reply_headers(&loaded.message);
    let raw = imip::answer_message(
        &ops::from_address(&loaded.account),
        &organizer,
        &loaded.invite,
        answer,
        in_reply_to.as_deref(),
        &references,
    )
    .map_err(CmdError::invalid)?;
    let provider = state.provider(&loaded.account.id).await?;
    provider
        .send_raw(&raw, Some(&loaded.message.thread_id))
        .await?;
    tracing::info!(account = %loaded.account.id, method = answer.method(), "invitation answered by email");
    Ok(())
}

/// Graph couldn't answer through the calendar (no permission, no linked
/// event, not supported): the email route still can.
fn graph_can_fall_back(e: &ProviderError) -> bool {
    matches!(
        e,
        ProviderError::Unsupported(_)
            | ProviderError::NotFound(_)
            | ProviderError::Http {
                status: 400 | 401 | 403 | 404,
                ..
            }
    )
}

/// Answer the invitation in a thread: Yes / Maybe / No, with an optional
/// note and proposed new time, through the account's route. Returns the
/// card as it is now. Local state (the row chip, the card) updates at once.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn respond_to_invite(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    calendar: State<'_, Arc<Calendar>>,
    account_id: String,
    thread_id: String,
    message_id: Option<String>,
    response: String,
    comment: Option<String>,
    proposal: Option<Proposal>,
) -> CmdResult<InviteCard> {
    if !gcal::valid_response(&response) {
        return Err(CmdError::invalid(
            "response must be accepted, tentative or declined",
        ));
    }
    let comment = comment
        .map(|c| c.trim().chars().take(2000).collect::<String>())
        .filter(|c| !c.is_empty());
    let proposal = proposal.map(|p| (p.start, p.end));
    if let Some((start, end)) = proposal {
        if end <= start {
            return Err(CmdError::invalid(
                "the proposed time must end after it starts",
            ));
        }
    }
    let loaded = load_invite(&state, &account_id, &thread_id, message_id.as_deref())
        .await?
        .ok_or_else(|| CmdError::not_found("there's no invitation in this conversation"))?;
    let before = card(&state, &loaded).await?;
    if !before.can_respond {
        return Err(CmdError::invalid(if before.method == "cancel" {
            "This event was cancelled"
        } else {
            "This invitation can't be answered (it's over, or you organize it)"
        }));
    }
    if proposal.is_some() && !before.can_propose {
        return Err(CmdError::invalid(
            "A new time can't be proposed for a whole recurring series",
        ));
    }
    let now = now_ms();
    let email_answer = imip::Answer {
        response: response.clone(),
        comment: comment.clone(),
        proposal,
        attendee: ops::from_address(&loaded.account),
        dtstamp_ms: now,
    };
    let via = match before.route.as_str() {
        "calendar" => {
            let services = state.services()?;
            let client = CalendarClient::new(services.auth.clone(), &loaded.account.email);
            let series = before.recurring
                && loaded.invite.recurrence_id.is_none()
                && before.event.recurring_event_id.is_some();
            gcal::respond_with(
                &client,
                &state.store,
                &before.event,
                &loaded.account.email,
                &response,
                comment.as_deref(),
                series,
            )
            .await?;
            // Google Calendar's API can't propose a time: that part goes by
            // email (a COUNTER the organizer's calendar shows as a proposal).
            if proposal.is_some() {
                send_email_answer(&state, &loaded, &email_answer).await?;
            }
            if series {
                calendar.wake_now();
            }
            "calendar"
        }
        "graph" => {
            let provider = state.provider(&account_id).await?;
            let answer = InvitationAnswer {
                response: response.clone(),
                comment: comment.clone(),
                proposal,
            };
            match provider
                .respond_to_invitation(&loaded.message.id, &answer)
                .await
            {
                Ok(()) => "graph",
                Err(e) if graph_can_fall_back(&e) => {
                    tracing::info!(account = %account_id, error = %e, "Graph can't answer this invitation; answering by email");
                    send_email_answer(&state, &loaded, &email_answer).await?;
                    "email"
                }
                Err(e) => return Err(e.into()),
            }
        }
        _ => {
            send_email_answer(&state, &loaded, &email_answer).await?;
            "email"
        }
    };
    let uid = loaded.invite.uid.clone().unwrap_or_default();
    let record = InviteResponse {
        response: response.clone(),
        via: via.to_string(),
        sequence: loaded.invite.sequence,
        comment,
        proposed_start: proposal.map(|p| p.0),
        proposed_end: proposal.map(|p| p.1),
        at: now,
    };
    let store = state.store.clone();
    let (a, u, r) = (account_id.clone(), uid.clone(), loaded.invite.recurrence_id);
    let threads = blocking(move || {
        store.record_invite_response(&a, &u, r, &record)?;
        Ok(store.invite_threads(&a, &u)?)
    })
    .await?;
    tracing::info!(account = %account_id, via, response = %response, proposed = proposal.is_some(), "invitation answered");
    let mut threads = threads;
    if !threads.contains(&thread_id) {
        threads.push(thread_id.clone());
    }
    state.emit_mail_changed(&account_id, threads);
    if via == "calendar" {
        emit_changed(&app, vec![account_id.clone()]);
    }
    card(&state, &loaded).await
}

/// `event_invite`: the card for a thread, or None.
pub async fn event_invite_card(
    state: &AppState,
    account_id: &str,
    thread_id: &str,
) -> CmdResult<Option<InviteCard>> {
    let Some(loaded) = load_invite(state, account_id, thread_id, None).await? else {
        return Ok(None);
    };
    let card = card(state, &loaded).await.inspect_err(|e| {
        tracing::warn!(account = %account_id, message = %loaded.message.id, code = ?e.code, error = %e.message, "invite: card failed");
    })?;
    Ok(Some(card))
}
