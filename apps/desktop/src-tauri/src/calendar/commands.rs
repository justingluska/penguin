//! Tauri side of the calendar: commands, the sync loop and connect flow.
//! Managed as `State<Arc<Calendar>>` next to AppState.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use penguin_core::{Account, CalendarEvent, PersonMeetings};
use penguin_gmail::calendar::{
    self as gcal, CalendarClient, CalendarWindow, CALENDAR_EVENTS_SCOPE, CALENDAR_READONLY_SCOPE,
};
use tauri::{AppHandle, Emitter, State};

use super::{
    account_status, CalendarChanged, CalendarStatus, EventDetail, EventThread, InviteCard,
    EVENT_CALENDAR_CHANGED,
};
use crate::error::{CmdError, CmdResult};
use crate::ops::now_ms;
use crate::state::{blocking, AppState};

/// Poll interval between sync rounds.
const POLL: Duration = Duration::from_secs(5 * 60);
/// Window focus triggers a round at most this often.
const FOCUS_DEBOUNCE: Duration = Duration::from_secs(60);
/// Backoff after rate limits and network errors: doubles up to the max.
const BACKOFF_MIN: Duration = Duration::from_secs(5 * 60);
const BACKOFF_MAX: Duration = Duration::from_secs(3600);
/// list_events spans at most this much (a month grid plus slack is ~6 weeks).
const MAX_RANGE_MS: i64 = 400 * 24 * 3600 * 1000;
const MAX_EVENTS: usize = 3000;

#[derive(Debug, Default)]
struct Live {
    syncing: bool,
    error: Option<String>,
    backoff_until: Option<Instant>,
    backoff: Option<Duration>,
}

/// Live sync state shared by the commands and the loop.
#[derive(Default)]
pub struct Calendar {
    wake: tokio::sync::Notify,
    live: Mutex<HashMap<String, Live>>,
    last_focus: Mutex<Option<Instant>>,
    /// Serializes rounds (the loop and connect_calendar's immediate sync).
    round: tokio::sync::Mutex<()>,
    /// Removed accounts (id → `added_at` of the removed incarnation, so the
    /// same address added again later syncs normally).
    removed: Mutex<HashMap<String, i64>>,
    removed_wake: tokio::sync::Notify,
}

impl Calendar {
    /// Sync now (e.g. after answering a whole series, whose instances
    /// Google updates on its side).
    pub fn wake_now(&self) {
        self.wake.notify_one();
    }

    /// The window regained focus: sync soon, but not on every alt-tab.
    pub fn on_focus(&self) {
        let mut last = self.last_focus.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_none_or(|t| t.elapsed() >= FOCUS_DEBOUNCE) {
            *last = Some(Instant::now());
            self.wake.notify_one();
        }
    }

    /// A removed account: never sync it again and abort a sync in flight,
    /// so nothing calls Google with the tokens sign-out is revoking. Its rows
    /// go with `Store::remove_account`.
    pub fn forget_account(&self, account: &Account) {
        self.removed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account.id.clone(), account.added_at);
        self.live(|l| l.remove(&account.id));
        self.removed_wake.notify_waiters();
    }

    fn is_removed(&self, account: &Account) -> bool {
        self.removed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&account.id)
            == Some(&account.added_at)
    }

    /// Resolves once `account` is removed.
    async fn removed_signal(&self, account: &Account) {
        loop {
            let notified = self.removed_wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_removed(account) {
                return;
            }
            notified.await;
        }
    }

    fn live<T>(&self, f: impl FnOnce(&mut HashMap<String, Live>) -> T) -> T {
        f(&mut self.live.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

pub fn init() -> Arc<Calendar> {
    Arc::new(Calendar::default())
}

pub fn spawn(app: AppHandle, state: Arc<AppState>, cal: Arc<Calendar>) {
    tauri::async_runtime::spawn(async move {
        // Let the mail sync's startup requests go first.
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            sync_round(&app, &state, &cal, None).await;
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = cal.wake.notified() => {}
            }
        }
    });
}

pub(super) fn has_scope(granted: &[String], scope: &str) -> bool {
    granted.iter().any(|g| g == scope)
}

pub(super) async fn granted_scopes(state: &AppState, email: &str) -> Vec<String> {
    let Ok(services) = state.services() else {
        return Vec::new();
    };
    let email = email.to_string();
    blocking(move || Ok(services.auth.granted_scopes(&email)))
        .await
        .unwrap_or_default()
}

/// What Settings shows for a failed round.
fn friendly(e: &penguin_gmail::Error) -> String {
    use penguin_gmail::Error as E;
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

pub(super) fn emit_changed(app: &AppHandle, account_ids: Vec<String>) {
    if account_ids.is_empty() {
        return;
    }
    if let Err(e) = app.emit(EVENT_CALENDAR_CHANGED, CalendarChanged { account_ids }) {
        tracing::warn!(error = %e, "failed to emit calendar-changed");
    }
}

/// Sync every connected account (or just `only`, ignoring its backoff).
async fn sync_round(
    app: &AppHandle,
    state: &Arc<AppState>,
    cal: &Arc<Calendar>,
    only: Option<&str>,
) {
    let _round = cal.round.lock().await;
    let Ok(services) = state.services() else {
        return;
    };
    let Ok(accounts) = state.accounts().await else {
        return;
    };
    let prefs = state.settings.get().calendar;
    let now = now_ms();
    let window = CalendarWindow::around(now, prefs.past_months, prefs.future_months);
    let mut changed = Vec::new();
    for account in accounts {
        // Google Calendar: Google accounts only.
        if !account.capabilities.calendar
            || only.is_some_and(|id| id != account.id)
            || cal.is_removed(&account)
        {
            continue;
        }
        if !has_scope(
            &granted_scopes(state, &account.email).await,
            CALENDAR_READONLY_SCOPE,
        ) {
            continue;
        }
        let waiting = cal.live(|l| {
            l.get(&account.id)
                .and_then(|x| x.backoff_until)
                .is_some_and(|t| Instant::now() < t)
        });
        if waiting && only.is_none() {
            continue;
        }
        cal.live(|l| l.entry(account.id.clone()).or_default().syncing = true);
        let client = CalendarClient::new(services.auth.clone(), &account.email);
        let sync = gcal::sync_account(
            &client,
            &state.store,
            &account.id,
            &account.email,
            window,
            now,
        );
        let result = tokio::select! {
            r = sync => r,
            _ = cal.removed_signal(&account) => {
                tracing::info!(account = %account.id, "calendar sync stopped: account removed");
                continue;
            }
        };
        cal.live(|l| {
            let live = l.entry(account.id.clone()).or_default();
            live.syncing = false;
            match &result {
                Ok(_) => {
                    live.error = None;
                    live.backoff = None;
                    live.backoff_until = None;
                }
                Err(e) => {
                    live.error = Some(friendly(e));
                    if matches!(
                        e,
                        penguin_gmail::Error::RateLimited | penguin_gmail::Error::Network(_)
                    ) {
                        let next = live
                            .backoff
                            .map_or(BACKOFF_MIN, |b| (b * 2).min(BACKOFF_MAX));
                        live.backoff = Some(next);
                        live.backoff_until = Some(Instant::now() + next);
                    }
                }
            }
        });
        match result {
            Ok(r) => {
                tracing::info!(account = %account.id, calendars = r.calendars, full = r.full, changed = r.changed, "calendar synced");
                if r.changed > 0 || r.full > 0 {
                    changed.push(account.id.clone());
                }
            }
            Err(e) => tracing::warn!(account = %account.id, error = %e, "calendar sync failed"),
        }
    }
    emit_changed(app, changed);
}

async fn status(state: &Arc<AppState>, cal: &Arc<Calendar>) -> CmdResult<CalendarStatus> {
    let mut accounts = state.accounts().await?;
    // Settings → Calendar lists only accounts that can have a Google Calendar.
    accounts.retain(|a| a.capabilities.calendar);
    let mut out = Vec::with_capacity(accounts.len());
    for a in accounts {
        let granted = granted_scopes(state, &a.email).await;
        let (read, rsvp) = (
            has_scope(&granted, CALENDAR_READONLY_SCOPE),
            has_scope(&granted, CALENDAR_EVENTS_SCOPE),
        );
        let store = state.store.clone();
        let id = a.id.clone();
        let mut s = blocking(move || Ok(account_status(&store, &id, read, rsvp)?)).await?;
        cal.live(|l| {
            if let Some(live) = l.get(&a.id) {
                s.syncing = live.syncing;
                s.error = live.error.clone();
            }
        });
        out.push(s);
    }
    Ok(CalendarStatus { accounts: out })
}

// ---------- commands ----------

type AppStateRef<'a> = State<'a, Arc<AppState>>;
type CalendarRef<'a> = State<'a, Arc<Calendar>>;

#[tauri::command]
pub async fn calendar_status(
    state: AppStateRef<'_>,
    calendar: CalendarRef<'_>,
) -> CmdResult<CalendarStatus> {
    status(state.inner(), calendar.inner()).await
}

/// "Connect calendar": a browser sign-in for this account asking for
/// `calendar.readonly` (and `calendar.events` with `rsvp`) on top of what it
/// already granted, then an immediate sync. For accounts whose calendar
/// didn't come with add-account (added before that, the setting was off, or
/// the box was unticked).
#[tauri::command]
pub async fn connect_calendar(
    app: AppHandle,
    state: AppStateRef<'_>,
    calendar: CalendarRef<'_>,
    account_id: String,
    rsvp: bool,
) -> CmdResult<CalendarStatus> {
    let account = state.account(&account_id).await?;
    if !account.capabilities.calendar {
        return Err(CmdError::invalid(
            "Calendar is available for Google accounts only",
        ));
    }
    let scopes: &[&str] = if rsvp {
        &[CALENDAR_READONLY_SCOPE, CALENDAR_EVENTS_SCOPE]
    } else {
        &[CALENDAR_READONLY_SCOPE]
    };
    let signed =
        crate::sign_in::interactive_sign_in(&app, &state, Some(&account.email), scopes).await?;
    if !signed.email.eq_ignore_ascii_case(&account.email) {
        return Err(CmdError::invalid(format!(
            "You signed in as {}; choose {} to connect its calendar",
            signed.email, account.email
        )));
    }
    let granted = granted_scopes(state.inner(), &account.email).await;
    if !has_scope(&granted, CALENDAR_READONLY_SCOPE) {
        return Err(CmdError::invalid(
            "Calendar access wasn't granted. Try again and leave the calendar box ticked on Google's screen",
        ));
    }
    if rsvp && !has_scope(&granted, CALENDAR_EVENTS_SCOPE) {
        return Err(CmdError::invalid(
            "RSVP access wasn't granted. Try again and tick \"View and edit events\" on Google's screen",
        ));
    }
    tracing::info!(account = %account.id, rsvp, "calendar connected");
    calendar.live(|l| {
        let live = l.entry(account.id.clone()).or_default();
        live.error = None;
        live.backoff_until = None;
    });
    sync_round(&app, state.inner(), calendar.inner(), Some(&account.id)).await;
    status(state.inner(), calendar.inner()).await
}

/// Calendar scopes for an add-account (`account` None) or reconnect sign-in:
/// read-only calendar when `calendar.connectOnSignIn` is on, plus whatever
/// calendar access a reconnecting account already had.
pub async fn sign_in_scopes(state: &AppState, account: Option<&Account>) -> Vec<&'static str> {
    let granted = match account {
        Some(a) => granted_scopes(state, &a.email).await,
        None => Vec::new(),
    };
    super::sign_in_scopes(state.settings.get().calendar.connect_on_sign_in, &granted)
}

/// After an add-account or reconnect sign-in that asked for `requested`:
/// when the calendar came with it, start its first sync in the background
/// (the command returns without waiting). When the user unticked it on
/// Google's screen, nothing happens: mail works and Settings → Calendar
/// still offers Connect.
pub async fn after_sign_in(
    app: &AppHandle,
    state: &Arc<AppState>,
    cal: &Arc<Calendar>,
    account: &Account,
    requested: &[&str],
) {
    if requested.is_empty() {
        return;
    }
    let granted = granted_scopes(state, &account.email).await;
    if !super::calendar_granted(requested, &granted) {
        tracing::info!(account = %account.id, "calendar not granted at sign-in; mail only");
        return;
    }
    tracing::info!(
        account = %account.id,
        rsvp = has_scope(&granted, CALENDAR_EVENTS_SCOPE),
        "calendar connected at sign-in"
    );
    cal.live(|l| {
        let live = l.entry(account.id.clone()).or_default();
        live.error = None;
        live.backoff = None;
        live.backoff_until = None;
    });
    let (app, state, cal, id) = (app.clone(), state.clone(), cal.clone(), account.id.clone());
    tauri::async_runtime::spawn(async move {
        sync_round(&app, &state, &cal, Some(&id)).await;
        // Settings → Calendar refreshes on this even when nothing synced
        // (e.g. the Calendar API is off and the account shows that error).
        emit_changed(&app, vec![id]);
    });
}

/// Show or hide one calendar. Hiding drops its events; showing syncs it.
#[tauri::command]
pub async fn set_calendar_selected(
    app: AppHandle,
    state: AppStateRef<'_>,
    calendar: CalendarRef<'_>,
    account_id: String,
    calendar_id: String,
    selected: bool,
) -> CmdResult<CalendarStatus> {
    let store = state.store.clone();
    let (a, c) = (account_id.clone(), calendar_id.clone());
    let found = blocking(move || Ok(store.set_calendar_selected(&a, &c, selected)?)).await?;
    if !found {
        return Err(CmdError::not_found(format!(
            "unknown calendar {calendar_id}"
        )));
    }
    emit_changed(&app, vec![account_id]);
    if selected {
        calendar.wake.notify_one();
    }
    status(state.inner(), calendar.inner()).await
}

/// Events overlapping [fromMs, toMs) from selected calendars; local only.
#[tauri::command]
pub async fn list_events(
    state: AppStateRef<'_>,
    from_ms: i64,
    to_ms: i64,
    account_ids: Option<Vec<String>>,
) -> CmdResult<Vec<CalendarEvent>> {
    if to_ms <= from_ms || to_ms - from_ms > MAX_RANGE_MS {
        return Err(CmdError::invalid(
            "the range must be positive and at most 400 days",
        ));
    }
    let store = state.store.clone();
    blocking(move || Ok(store.list_events(from_ms, to_ms, account_ids.as_deref(), MAX_EVENTS)?))
        .await
}

/// One event with its calendar and invitation thread; local only.
#[tauri::command]
pub async fn get_event(
    state: AppStateRef<'_>,
    account_id: String,
    calendar_id: String,
    event_id: String,
) -> CmdResult<Option<EventDetail>> {
    let store = state.store.clone();
    blocking(move || {
        let Some(event) = store.get_event(&account_id, &calendar_id, &event_id)? else {
            return Ok(None);
        };
        let calendar = store
            .list_calendars(Some(std::slice::from_ref(&account_id)))?
            .into_iter()
            .find(|c| c.id == calendar_id);
        let thread = store
            .event_thread(&event)?
            .map(|(thread_id, subject)| EventThread {
                account_id: account_id.clone(),
                thread_id,
                subject,
            });
        Ok(Some(EventDetail {
            event,
            calendar,
            thread,
        }))
    })
    .await
}

/// The invitation card for a thread, or null when it carries none. Reads
/// the newest calendar part (fetched once, then from the attachment cache)
/// and remembers it for the row chip (`calendar::invites`).
#[tauri::command]
pub async fn event_invite(
    state: AppStateRef<'_>,
    account_id: String,
    thread_id: String,
) -> CmdResult<Option<InviteCard>> {
    super::invites::event_invite_card(state.inner(), &account_id, &thread_id).await
}

/// "Last / next meeting with X" for the person card; local only.
#[tauri::command]
pub async fn person_meetings(state: AppStateRef<'_>, email: String) -> CmdResult<PersonMeetings> {
    let store = state.store.clone();
    blocking(move || Ok(store.person_meetings(&email, now_ms())?)).await
}

/// Accept / Maybe / Decline one event (needs the RSVP opt-in). Google tells
/// the organizer, as when answering in Google Calendar.
#[tauri::command]
pub async fn respond_to_event(
    app: AppHandle,
    state: AppStateRef<'_>,
    account_id: String,
    calendar_id: String,
    event_id: String,
    response: String,
) -> CmdResult<CalendarEvent> {
    if !gcal::valid_response(&response) {
        return Err(CmdError::invalid(
            "response must be accepted, tentative or declined",
        ));
    }
    let account = state.account(&account_id).await?;
    if !has_scope(
        &granted_scopes(state.inner(), &account.email).await,
        CALENDAR_EVENTS_SCOPE,
    ) {
        return Err(CmdError::invalid(
            "Turn on RSVP for this account first (Settings → Calendar)",
        ));
    }
    let store = state.store.clone();
    let (a, c, e) = (account_id.clone(), calendar_id.clone(), event_id.clone());
    let event = blocking(move || Ok(store.get_event(&a, &c, &e)?))
        .await?
        .ok_or_else(|| CmdError::not_found("that event isn't on your calendar"))?;
    let services = state.services()?;
    let client = CalendarClient::new(services.auth.clone(), &account.email);
    let updated = gcal::respond(&client, &state.store, &event, &account.email, &response).await?;
    tracing::info!(account = %account_id, response = %response, "event response sent");
    emit_changed(&app, vec![account_id]);
    Ok(updated)
}

/// Sync every connected account now (Settings, pull-to-refresh).
#[tauri::command]
pub async fn calendar_sync_now(calendar: CalendarRef<'_>) -> CmdResult<()> {
    calendar.wake.notify_one();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(added_at: i64) -> Account {
        Account {
            id: "ada@northwind.example".into(),
            email: "ada@northwind.example".into(),
            display_name: None,
            nickname: None,
            color: "#123456".into(),
            added_at,
            ..Account::default()
        }
    }

    #[tokio::test]
    async fn removing_an_account_stops_its_sync_at_once() {
        let cal = Arc::new(Calendar::default());
        let a = account(1);
        assert!(!cal.is_removed(&a));
        // A sync in flight: its removal signal is pending until removal.
        let waiter = {
            let (cal, a) = (cal.clone(), a.clone());
            tokio::spawn(async move { cal.removed_signal(&a).await })
        };
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        cal.live(|l| l.entry(a.id.clone()).or_default().error = Some("x".into()));
        cal.forget_account(&a);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the in-flight sync is aborted")
            .unwrap();
        assert!(cal.is_removed(&a));
        assert!(
            cal.live(|l| l.get(&a.id).is_none()),
            "its live state is gone"
        );
        // Removed before the round starts: it resolves immediately.
        tokio::time::timeout(Duration::from_millis(100), cal.removed_signal(&a))
            .await
            .unwrap();
        // The same address added again later syncs normally.
        assert!(!cal.is_removed(&account(2)));
    }
}
