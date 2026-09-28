//! Google Calendar: read-only sync of each connected account's calendars into
//! the local store, plus the optional RSVP write. OWNER: calendar agent.
//!
//! - Opt-in per account: `calendar.readonly` is requested incrementally
//!   (Settings → Calendar → Connect). RSVP needs `calendar.events` as a
//!   second, separate opt-in.
//! - Per selected calendar: one full `events.list` over the window
//!   (`singleEvents=true`, so recurring series arrive as instances), then
//!   incremental `events.list?syncToken=…` rounds. A 410 GONE drops the
//!   token and the next round is a full sync. Cancelled items delete rows;
//!   changes outside the window are ignored. Google's generated calendars
//!   (holidays, birthdays) reject their sync tokens, so they're fully
//!   re-listed once a day instead.
//! - The window (default: 2 years back, 1 year ahead) slides: stored events
//!   that end before its start are pruned, and once its end has moved a week
//!   past what was fetched, the calendar is fully re-listed so instances of
//!   recurring series that entered the window show up.
//! - Calendar API quotas are separate from Gmail's (per-project queries per
//!   day, per-user queries per minute). A round costs one calendarList call
//!   plus one events.list per selected calendar, usually one page each; the
//!   app polls every ~5 minutes and on window focus (debounced).
//!
//! The flows are generic over [`CalendarApi`] so they can be tested without
//! Google; [`CalendarClient`] is the real implementation.

use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use penguin_core::store::CalendarCursor;
use penguin_core::{Address, CalendarEvent, CalendarInfo, EventAttendee, Store};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::auth::AuthManager;
use crate::{Error, Result};

/// Read-only access to calendars and events (Google: sensitive, not restricted).
pub const CALENDAR_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
/// Write access to events, only for RSVP (a separate opt-in).
pub const CALENDAR_EVENTS_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";

const BASE: &str = "https://www.googleapis.com/calendar/v3";
const PAGE_SIZE: u32 = 2500;
/// Pages per events.list pass; far beyond any real calendar (2500 each).
const MAX_PAGES: usize = 40;
const MAX_DESCRIPTION: usize = 8 * 1024;
const DAY_MS: i64 = 24 * 3600 * 1000;
/// Re-list a calendar in full once the window's end has moved this far past
/// what was last fetched (new recurring instances only arrive that way).
const WINDOW_SLIDE_MS: i64 = 7 * DAY_MS;

// ---------- wire types ----------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarListPage {
    pub items: Vec<WireCalendar>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WireCalendar {
    pub id: String,
    pub summary: String,
    pub summary_override: Option<String>,
    pub background_color: Option<String>,
    pub selected: bool,
    pub primary: bool,
    pub access_role: String,
    pub time_zone: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EventsPage {
    pub items: Vec<WireEvent>,
    pub next_page_token: Option<String>,
    pub next_sync_token: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WireEvent {
    pub id: String,
    pub status: String,
    pub html_link: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: Option<WireTime>,
    pub end: Option<WireTime>,
    pub organizer: Option<WirePerson>,
    pub attendees: Vec<WirePerson>,
    pub recurring_event_id: Option<String>,
    #[serde(rename = "iCalUID")]
    pub ical_uid: Option<String>,
    pub updated: Option<String>,
    pub hangout_link: Option<String>,
    pub conference_data: Option<WireConference>,
    pub event_type: Option<String>,
    pub transparency: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WireTime {
    pub date: Option<String>,
    pub date_time: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WirePerson {
    pub email: String,
    pub display_name: Option<String>,
    pub response_status: Option<String>,
    pub organizer: bool,
    #[serde(rename = "self")]
    pub is_self: bool,
    pub optional: bool,
    pub resource: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WireConference {
    pub entry_points: Vec<WireEntryPoint>,
    pub conference_solution: Option<WireSolution>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WireEntryPoint {
    pub entry_point_type: String,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WireSolution {
    pub name: Option<String>,
}

/// One events.list request: either `sync_token` or the window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventsQuery {
    pub sync_token: Option<String>,
    pub time_min: Option<i64>,
    pub time_max: Option<i64>,
    pub page_token: Option<String>,
}

// ---------- API ----------

/// The Calendar endpoints the flows need.
#[allow(async_fn_in_trait)]
pub trait CalendarApi {
    async fn calendar_list(&self, page_token: Option<&str>) -> Result<CalendarListPage>;
    async fn list_events(&self, calendar_id: &str, query: &EventsQuery) -> Result<EventsPage>;
    /// events.get, as raw JSON (the RSVP patch must echo every attendee).
    async fn get_event_json(&self, calendar_id: &str, event_id: &str) -> Result<serde_json::Value>;
    /// events.patch with `body`; returns the updated event.
    async fn patch_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        body: &serde_json::Value,
    ) -> Result<WireEvent>;
}

/// The token for this sync was invalidated (410 GONE: `fullSyncRequired`).
pub fn is_gone(e: &Error) -> bool {
    matches!(e, Error::Http { status: 410, .. })
}

/// 403 because the Calendar API is off in the user's own Cloud project.
pub fn is_api_disabled(e: &Error) -> bool {
    matches!(e, Error::Http { status: 403, body }
        if body.contains("SERVICE_DISABLED") || body.contains("accessNotConfigured"))
}

/// 401/403 for a missing scope (not connected, or access revoked).
pub fn is_not_granted(e: &Error) -> bool {
    match e {
        Error::Http { status: 401, .. } => true,
        Error::Http { status: 403, body } => {
            !is_api_disabled(e)
                && (body.contains("insufficient")
                    || body.contains("ACCESS_TOKEN_SCOPE_INSUFFICIENT")
                    || body.contains("PERMISSION_DENIED")
                    || body.contains("forbidden"))
        }
        _ => false,
    }
}

fn http() -> reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::user_agent())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client")
    })
    .clone()
}

/// Calendar REST for one account.
#[derive(Clone)]
pub struct CalendarClient {
    http: reqwest::Client,
    auth: AuthManager,
    email: String,
}

const MAX_ATTEMPTS: u32 = 4;

impl CalendarClient {
    pub fn new(auth: AuthManager, account_email: &str) -> CalendarClient {
        CalendarClient {
            http: http(),
            auth,
            email: account_email.to_string(),
        }
    }

    /// One call with a token refresh on 401 and backoff on 5xx. Throttling
    /// (429 / rateLimitExceeded) is returned as `RateLimited` at once: the
    /// sync loop backs off as a whole rather than retrying in place.
    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<Vec<u8>> {
        let url = format!("{BASE}{path}");
        let mut attempt = 0;
        let mut refreshed = false;
        loop {
            let token = self.auth.access_token(&self.email).await?;
            let mut req = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(token)
                .query(query);
            if let Some(b) = body {
                req = req.json(b);
            }
            let err = match req.send().await {
                Err(e) => Error::Network(e.without_url().to_string()),
                Ok(resp) => {
                    let status = resp.status();
                    let bytes = resp
                        .bytes()
                        .await
                        .map_err(|e| Error::Network(e.without_url().to_string()))?;
                    if status.is_success() {
                        return Ok(bytes.to_vec());
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    if status == StatusCode::TOO_MANY_REQUESTS
                        || (status == StatusCode::FORBIDDEN
                            && (text.contains("ateLimitExceeded")
                                || text.contains("quotaExceeded")))
                    {
                        return Err(Error::RateLimited);
                    }
                    if status == StatusCode::UNAUTHORIZED && !refreshed {
                        refreshed = true;
                        self.auth.invalidate_access_token(&self.email).await;
                        continue;
                    }
                    let err = Error::Http {
                        status: status.as_u16(),
                        body: truncate(&text),
                    };
                    if !status.is_server_error() {
                        return Err(err);
                    }
                    err
                }
            };
            attempt += 1;
            if attempt >= MAX_ATTEMPTS {
                return Err(err);
            }
            let wait = Duration::from_millis(500 * (1 << attempt) + fastrand::u64(0..500));
            tracing::debug!(attempt, ?wait, error = %err, "calendar retry");
            tokio::time::sleep(wait).await;
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let bytes = self.call(Method::GET, path, query, None).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("calendar {path}: bad json: {e}")))
    }
}

fn truncate(body: &str) -> String {
    let mut end = body.len().min(500);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_string()
}

/// Path segment escaping for calendar and event ids (ids contain `@`, `#`).
fn seg(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn rfc3339(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl CalendarApi for CalendarClient {
    async fn calendar_list(&self, page_token: Option<&str>) -> Result<CalendarListPage> {
        let mut q = vec![("maxResults", "250".to_string())];
        if let Some(t) = page_token {
            q.push(("pageToken", t.to_string()));
        }
        self.get("/users/me/calendarList", &q).await
    }

    async fn list_events(&self, calendar_id: &str, query: &EventsQuery) -> Result<EventsPage> {
        let mut q = vec![
            ("singleEvents", "true".to_string()),
            ("maxResults", PAGE_SIZE.to_string()),
        ];
        match &query.sync_token {
            Some(t) => q.push(("syncToken", t.clone())),
            None => {
                if let Some(min) = query.time_min {
                    q.push(("timeMin", rfc3339(min)));
                }
                if let Some(max) = query.time_max {
                    q.push(("timeMax", rfc3339(max)));
                }
            }
        }
        if let Some(p) = &query.page_token {
            q.push(("pageToken", p.clone()));
        }
        self.get(&format!("/calendars/{}/events", seg(calendar_id)), &q)
            .await
    }

    async fn get_event_json(&self, calendar_id: &str, event_id: &str) -> Result<serde_json::Value> {
        self.get(
            &format!("/calendars/{}/events/{}", seg(calendar_id), seg(event_id)),
            &[],
        )
        .await
    }

    async fn patch_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        body: &serde_json::Value,
    ) -> Result<WireEvent> {
        let path = format!("/calendars/{}/events/{}", seg(calendar_id), seg(event_id));
        // The organizer hears about the new response, as from Google Calendar.
        let bytes = self
            .call(
                Method::PATCH,
                &path,
                &[("sendUpdates", "all".to_string())],
                Some(body),
            )
            .await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("calendar patch: bad json: {e}")))
    }
}

// ---------- conversion ----------

/// The sync window, unix ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarWindow {
    pub start: i64,
    pub end: i64,
}

impl CalendarWindow {
    /// `past_months` back and `future_months` ahead of `now_ms`, snapped to
    /// local midnight so the bounds don't move on every round.
    pub fn around(now_ms: i64, past_months: u32, future_months: u32) -> CalendarWindow {
        let today = DateTime::<Utc>::from_timestamp_millis(now_ms)
            .unwrap_or_default()
            .with_timezone(&Local)
            .date_naive();
        let start = today
            .checked_sub_months(chrono::Months::new(past_months))
            .unwrap_or(today);
        let end = today
            .checked_add_months(chrono::Months::new(future_months))
            .unwrap_or(today);
        CalendarWindow {
            start: local_midnight_ms(start),
            end: local_midnight_ms(end),
        }
    }
}

/// Unix ms of local midnight on `date` (the first valid instant that day).
pub fn local_midnight_ms(date: NaiveDate) -> i64 {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    match Local.from_local_datetime(&midnight).earliest() {
        Some(t) => t.timestamp_millis(),
        // A DST gap at midnight: the day starts an hour later.
        None => Local
            .from_local_datetime(&(midnight + chrono::Duration::hours(1)))
            .earliest()
            .map_or_else(
                || midnight.and_utc().timestamp_millis(),
                |t| t.timestamp_millis(),
            ),
    }
}

fn parse_time(t: &WireTime) -> Option<(i64, Option<String>)> {
    if let Some(dt) = &t.date_time {
        return DateTime::parse_from_rfc3339(dt)
            .ok()
            .map(|d| (d.timestamp_millis(), None));
    }
    let d = t.date.as_deref()?;
    let date = NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
    Some((local_midnight_ms(date), Some(d.to_string())))
}

fn https_only(url: Option<&str>) -> Option<String> {
    url.map(str::trim)
        .filter(|u| u.starts_with("https://") && !u.contains(char::is_whitespace))
        .map(str::to_string)
}

/// The kind of a video-call link, by host.
pub fn conference_kind(url: &str) -> Option<&'static str> {
    let host = url
        .strip_prefix("https://")?
        .split(['/', '?', '#'])
        .next()?
        .to_ascii_lowercase();
    let is = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    if is("meet.google.com") {
        Some("meet")
    } else if is("zoom.us") || is("zoomgov.com") {
        Some("zoom")
    } else if is("teams.microsoft.com") || is("teams.live.com") {
        Some("teams")
    } else if is("webex.com") {
        Some("webex")
    } else if is("whereby.com") || is("around.co") || is("chime.aws") {
        Some("other")
    } else {
        None
    }
}

/// The first https video-call link in free text (location, description).
pub fn find_conference_link(text: &str) -> Option<(String, &'static str)> {
    let mut rest = text;
    while let Some(i) = rest.find("https://") {
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')' | ']'))
            .unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(['.', ',', ';']);
        if let Some(kind) = conference_kind(url) {
            return Some((url.to_string(), kind));
        }
        rest = &tail[end.max(8)..];
    }
    None
}

/// Descriptions may carry Google's limited HTML; the UI shows text only.
pub fn description_text(s: &str) -> String {
    let text = if s.contains('<') {
        penguin_core::text::html_to_text(s)
    } else {
        s.to_string()
    };
    let mut text = text.trim().to_string();
    if text.len() > MAX_DESCRIPTION {
        let mut end = MAX_DESCRIPTION;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

fn attendee(p: &WirePerson, account_email: &str) -> EventAttendee {
    let email = p.email.trim().to_lowercase();
    EventAttendee {
        is_self: p.is_self || email == account_email,
        email,
        name: p.display_name.clone().filter(|n| !n.trim().is_empty()),
        response: p
            .response_status
            .clone()
            .unwrap_or_else(|| "needsAction".into()),
        organizer: p.organizer,
        optional: p.optional,
        resource: p.resource,
    }
}

/// A stored event from Google's copy; None for cancelled items and ones
/// with nothing to show (working-location markers, no start).
pub fn to_event(
    account_id: &str,
    account_email: &str,
    calendar_id: &str,
    w: &WireEvent,
) -> Option<CalendarEvent> {
    if w.status == "cancelled" || w.event_type.as_deref() == Some("workingLocation") {
        return None;
    }
    let (start, start_date) = parse_time(w.start.as_ref()?)?;
    let (end, end_date) = w
        .end
        .as_ref()
        .and_then(parse_time)
        .unwrap_or((start, start_date.clone()));
    let account_email = account_email.to_lowercase();
    let attendees: Vec<EventAttendee> = w
        .attendees
        .iter()
        .map(|p| attendee(p, &account_email))
        .collect();
    let my_response = attendees
        .iter()
        .find(|a| a.is_self)
        .map(|a| a.response.clone());
    let description = description_text(w.description.as_deref().unwrap_or(""));
    let location = w.location.clone().unwrap_or_default();
    let mut conference = https_only(w.hangout_link.as_deref()).map(|u| (u, "meet"));
    if let Some(cd) = &w.conference_data {
        if let Some(ep) = cd
            .entry_points
            .iter()
            .find(|e| e.entry_point_type == "video")
        {
            if let Some(u) = https_only(Some(&ep.uri)) {
                let kind = conference_kind(&u).unwrap_or("other");
                conference = Some((u, kind));
            }
        }
    }
    let conference = conference
        .or_else(|| find_conference_link(&location))
        .or_else(|| find_conference_link(&description));
    Some(CalendarEvent {
        account_id: account_id.to_string(),
        calendar_id: calendar_id.to_string(),
        id: w.id.clone(),
        ical_uid: w.ical_uid.clone().filter(|u| !u.is_empty()),
        status: if w.status.is_empty() {
            "confirmed".into()
        } else {
            w.status.clone()
        },
        summary: w.summary.clone().unwrap_or_default().trim().to_string(),
        description,
        location,
        start,
        end: end.max(start),
        all_day: start_date.is_some(),
        start_date,
        end_date,
        organizer: w.organizer.as_ref().map(|o| Address {
            email: o.email.trim().to_lowercase(),
            name: o.display_name.clone().filter(|n| !n.trim().is_empty()),
        }),
        attendees,
        my_response,
        html_link: https_only(w.html_link.as_deref()),
        conference_url: conference.as_ref().map(|c| c.0.clone()),
        conference_kind: conference.map(|c| c.1.to_string()),
        recurring_event_id: w.recurring_event_id.clone(),
        free: w.transparency.as_deref() == Some("transparent"),
        updated: w
            .updated
            .as_deref()
            .and_then(|u| DateTime::parse_from_rfc3339(u).ok())
            .map_or(0, |d| d.timestamp_millis()),
    })
}

fn to_info(account_id: &str, c: &WireCalendar) -> CalendarInfo {
    CalendarInfo {
        account_id: account_id.to_string(),
        id: c.id.clone(),
        summary: c
            .summary_override
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| c.summary.clone()),
        color: c
            .background_color
            .clone()
            .filter(|c| c.len() == 7 && c.starts_with('#')),
        // The primary calendar is always on to begin with.
        selected: c.selected || c.primary,
        primary: c.primary,
        access_role: if c.access_role.is_empty() {
            "reader".into()
        } else {
            c.access_role.clone()
        },
    }
}

// ---------- sync ----------

/// Store calls are blocking: run them off the async threads.
async fn db<T: Send + 'static>(
    store: &Store,
    f: impl FnOnce(&Store) -> penguin_core::Result<T> + Send + 'static,
) -> Result<T> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| Error::Other(format!("store task failed: {e}")))?
        .map_err(Error::from)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub calendars: usize,
    /// Calendars that were listed in full this round.
    pub full: usize,
    /// Rows written or deleted.
    pub changed: usize,
}

/// Does this calendar need a full listing for `window`?
fn needs_full(cursor: &CalendarCursor, window: CalendarWindow) -> bool {
    let (Some(_), Some(start), Some(end)) =
        (&cursor.sync_token, cursor.window_start, cursor.window_end)
    else {
        return true;
    };
    // Grew backwards, or the end slid far enough to miss new instances.
    start > window.start + DAY_MS || end + WINDOW_SLIDE_MS < window.end
}

/// Google's generated calendars (holidays, birthdays, week numbers, under
/// `group.v.calendar.google.com`) answer every incremental round with 410,
/// even for the token their own full listing just issued
/// (issuetracker.google.com/issues/372283558). They change a few times a
/// year, so they skip sync tokens and are re-listed at most once a day.
fn generated(calendar_id: &str) -> bool {
    calendar_id.ends_with("@group.v.calendar.google.com")
}

/// Is a generated calendar's last full listing too old or too narrow?
fn generated_stale(cursor: &CalendarCursor, window: CalendarWindow, now_ms: i64) -> bool {
    let (Some(at), Some(start), Some(end)) =
        (cursor.synced_at, cursor.window_start, cursor.window_end)
    else {
        return true;
    };
    now_ms - at >= DAY_MS || start > window.start + DAY_MS || end + WINDOW_SLIDE_MS < window.end
}

fn in_window(e: &CalendarEvent, window: CalendarWindow) -> bool {
    e.end > window.start && e.start < window.end
}

/// Sync every selected calendar of one account. Stops at the first error
/// (rate limits and missing scopes affect the whole account alike); what
/// was stored before it stays.
pub async fn sync_account<A: CalendarApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    account_email: &str,
    window: CalendarWindow,
    now_ms: i64,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();

    let mut listed = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let p = api.calendar_list(page.as_deref()).await?;
        listed.extend(p.items.into_iter().filter(|c| !c.deleted));
        match p.next_page_token.filter(|t| !t.is_empty()) {
            Some(t) => page = Some(t),
            None => break,
        }
    }
    let infos: Vec<CalendarInfo> = listed.iter().map(|c| to_info(account_id, c)).collect();
    let zones: Vec<Option<String>> = listed.iter().map(|c| c.time_zone.clone()).collect();
    let id = account_id.to_string();
    let (pruned, calendars) = db(store, move |s| {
        s.replace_calendars(&id, &infos, &zones)?;
        let pruned = s.prune_events_before(&id, window.start)?;
        Ok((pruned, s.list_calendars(Some(std::slice::from_ref(&id)))?))
    })
    .await?;
    report.changed += pruned;

    for cal in calendars
        .iter()
        // Free/busy-only calendars have no titles to show or search.
        .filter(|c| c.selected && c.access_role != "freeBusyReader")
    {
        report.calendars += 1;
        let (id, cal_id) = (account_id.to_string(), cal.id.clone());
        let cursor = db(store, move |s| s.calendar_cursor(&id, &cal_id)).await?;
        if generated(&cal.id) {
            if !generated_stale(&cursor, window, now_ms) {
                continue;
            }
        } else if !needs_full(&cursor, window) {
            match incremental(
                api,
                store,
                account_id,
                account_email,
                &cal.id,
                &cursor,
                window,
                now_ms,
            )
            .await
            {
                Ok(n) => {
                    report.changed += n;
                    continue;
                }
                Err(e) if is_gone(&e) => {
                    tracing::info!(account = %account_id, "calendar sync token expired; full sync");
                }
                Err(e) => return Err(e),
            }
        }
        report.changed += full(
            api,
            store,
            account_id,
            account_email,
            &cal.id,
            window,
            now_ms,
        )
        .await?;
        report.full += 1;
    }
    Ok(report)
}

async fn full<A: CalendarApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    account_email: &str,
    calendar_id: &str,
    window: CalendarWindow,
    now_ms: i64,
) -> Result<usize> {
    let mut events = Vec::new();
    let mut query = EventsQuery {
        sync_token: None,
        time_min: Some(window.start),
        time_max: Some(window.end),
        page_token: None,
    };
    let mut sync_token = None;
    for _ in 0..MAX_PAGES {
        let page = api.list_events(calendar_id, &query).await?;
        events.extend(
            page.items
                .iter()
                .filter_map(|w| to_event(account_id, account_email, calendar_id, w)),
        );
        match page.next_page_token.filter(|t| !t.is_empty()) {
            Some(t) => query.page_token = Some(t),
            None => {
                sync_token = page.next_sync_token;
                break;
            }
        }
    }
    let n = events.len();
    let cursor = CalendarCursor {
        sync_token,
        window_start: Some(window.start),
        window_end: Some(window.end),
        synced_at: Some(now_ms),
    };
    let (id, cal_id) = (account_id.to_string(), calendar_id.to_string());
    db(store, move |s| {
        s.replace_calendar_events(&id, &cal_id, &events, &cursor)
    })
    .await?;
    Ok(n)
}

#[allow(clippy::too_many_arguments)]
async fn incremental<A: CalendarApi>(
    api: &A,
    store: &Store,
    account_id: &str,
    account_email: &str,
    calendar_id: &str,
    cursor: &CalendarCursor,
    window: CalendarWindow,
    now_ms: i64,
) -> Result<usize> {
    let mut upserts = Vec::new();
    let mut deletes = Vec::new();
    let mut query = EventsQuery {
        sync_token: cursor.sync_token.clone(),
        ..Default::default()
    };
    let mut next_token = None;
    for _ in 0..MAX_PAGES {
        let page = api.list_events(calendar_id, &query).await?;
        for w in &page.items {
            match to_event(account_id, account_email, calendar_id, w) {
                Some(e) if in_window(&e, window) => upserts.push(e),
                // Cancelled, moved out of the window, or nothing to show.
                _ => deletes.push(w.id.clone()),
            }
        }
        match page.next_page_token.filter(|t| !t.is_empty()) {
            Some(t) => query.page_token = Some(t),
            None => {
                next_token = page.next_sync_token;
                break;
            }
        }
    }
    let Some(token) = next_token else {
        // Ran past MAX_PAGES: a full listing is cheaper than carrying on.
        return Err(Error::Http {
            status: 410,
            body: "too many changes".into(),
        });
    };
    let next = CalendarCursor {
        sync_token: Some(token),
        window_start: cursor.window_start,
        window_end: cursor.window_end,
        synced_at: Some(now_ms),
    };
    let (id, cal_id) = (account_id.to_string(), calendar_id.to_string());
    db(store, move |s| {
        s.apply_calendar_changes(&id, &cal_id, &upserts, &deletes, &next)
    })
    .await
}

// ---------- RSVP ----------

/// accepted | tentative | declined
pub fn valid_response(r: &str) -> bool {
    matches!(r, "accepted" | "tentative" | "declined")
}

/// Set the account's own response on one event (one instance of a series),
/// then store Google's updated copy. Needs `calendar.events`.
pub async fn respond<A: CalendarApi>(
    api: &A,
    store: &Store,
    event: &CalendarEvent,
    account_email: &str,
    response: &str,
) -> Result<CalendarEvent> {
    respond_with(api, store, event, account_email, response, None, false)
        .await
        .map(|e| e.unwrap_or_else(|| event.clone()))
}

/// [`respond`] with a note to the organizer (the attendee's `comment`;
/// None leaves an earlier note as it is, Some("") clears it) and, with
/// `series`, for the whole recurring series (the master event
/// `recurring_event_id`) instead of this instance. Returns the updated
/// instance, or None for a series (its instances arrive with the next sync).
pub async fn respond_with<A: CalendarApi>(
    api: &A,
    store: &Store,
    event: &CalendarEvent,
    account_email: &str,
    response: &str,
    comment: Option<&str>,
    series: bool,
) -> Result<Option<CalendarEvent>> {
    if !valid_response(response) {
        return Err(Error::Other(format!("unknown response {response}")));
    }
    let me = account_email.to_lowercase();
    let target = match (&event.recurring_event_id, series) {
        (Some(master), true) => master.clone(),
        _ => event.id.clone(),
    };
    // Echo Google's current attendee list: patch replaces the whole array.
    let current = api.get_event_json(&event.calendar_id, &target).await?;
    let mut attendees = current
        .get("attendees")
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    let mut found = false;
    for a in attendees.iter_mut() {
        let is_me = a.get("self").and_then(|v| v.as_bool()) == Some(true)
            || a.get("email")
                .and_then(|v| v.as_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(&me));
        if is_me {
            a["responseStatus"] = serde_json::Value::String(response.into());
            if let Some(c) = comment {
                let c = c.trim();
                if c.is_empty() {
                    if let Some(o) = a.as_object_mut() {
                        o.remove("comment");
                    }
                } else {
                    a["comment"] = serde_json::Value::String(c.into());
                }
            }
            found = true;
        }
    }
    if !found {
        return Err(Error::Other(
            "you're not on this event's guest list, so there's nothing to answer".into(),
        ));
    }
    let body = serde_json::json!({ "attendees": attendees });
    let updated = api.patch_event(&event.calendar_id, &target, &body).await?;
    if target != event.id {
        return Ok(None);
    }
    let stored = to_event(&event.account_id, &me, &event.calendar_id, &updated)
        .ok_or_else(|| Error::Other("Google returned a cancelled event".into()))?;
    let copy = stored.clone();
    db(store, move |s| s.upsert_calendar_event(&copy)).await?;
    Ok(Some(stored))
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
