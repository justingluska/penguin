//! User settings: `<config dir>/settings.json`, read once at startup and
//! rewritten atomically (0600) on every change. Mirrored in
//! `apps/desktop/src/lib/types.ts` (Settings, SettingsPatch) — change both
//! together. Every field has a serde default, so a file written by an older
//! build (or a hand-edited one missing keys) still loads.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use penguin_core::Message;
use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";
/// Payload: the full `Settings` after the change.
pub const EVENT_SETTINGS_CHANGED: &str = "penguin://settings-changed";
/// Bound on the per-sender image allowlist (keeps the file and lookups small).
const MAX_TRUSTED_SENDERS: usize = 5000;
/// Bound on account profiles (⌃1–⌃9 reach the first nine).
pub const MAX_PROFILES: usize = 20;
const MAX_PROFILE_NAME: usize = 40;
const MAX_PROFILE_ACCOUNTS: usize = 50;
const MAX_EMOJI_CHARS: usize = 8;
const DEFAULT_PROFILE_COLOR: &str = "#8d5ffe";
/// Bound on the accounts hidden from All Inboxes.
const MAX_HIDDEN_FROM_ALL: usize = 100;
/// Bound on the saved account order.
const MAX_ACCOUNT_ORDER: usize = 100;
/// Bound on the per-account notification switches.
const MAX_NOTIFY_ACCOUNTS: usize = 100;
/// Bound on unfinished Add account setups, and on their saved error text.
const MAX_PENDING_SETUPS: usize = 20;
const MAX_PENDING_ERROR_CHARS: usize = 300;
const MAX_PENDING_STEP_CHARS: usize = 40;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThemeSetting {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
}

/// How the conversation list's rows look (Settings → General → List style).
/// The row heights that go with each live in the UI (features/inbox/rowMetrics.ts).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ListStyle {
    /// One accent in the column (the unread dot), monochrome monograms, the
    /// account as a dot on the avatar, read rows that step back.
    #[default]
    Quiet,
    /// The original look: colored monograms and an account bar on the edge.
    Classic,
    /// Unread rows on an accent wash in bold, read rows muted.
    Contrast,
    /// Each conversation a rounded card, with air between them.
    Cards,
    /// Larger faces, the sender as a headline and two lines of preview.
    Mail,
}

/// An unknown list style (a newer build's, or a typo) falls back to Quiet.
fn lenient_list_style<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ListStyle, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RemoteImages {
    /// Blocked until "Load images" (or a trusted sender).
    #[default]
    Ask,
    /// Always loaded when a message is rendered.
    Always,
    /// Never loaded; load_remote_images refuses.
    Never,
}

/// What a trackpad swipe on a list row does (swipe actions in the UI).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SwipeAction {
    ToggleRead,
    Star,
    Archive,
    Trash,
    None,
}

/// Unknown swipe action names fall back to that field's default.
fn lenient_swipe<'de, D: serde::Deserializer<'de>>(
    d: D,
    fallback: SwipeAction,
) -> Result<SwipeAction, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or(fallback))
}
fn lenient_swipe_right<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SwipeAction, D::Error> {
    lenient_swipe(d, SwipeAction::ToggleRead)
}
fn lenient_swipe_left<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SwipeAction, D::Error> {
    lenient_swipe(d, SwipeAction::Archive)
}
fn lenient_swipe_left_long<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<SwipeAction, D::Error> {
    lenient_swipe(d, SwipeAction::Trash)
}

/// Sidebar tint (apps/desktop/src/styles/themes.css). Wire names match the
/// `[data-sidebar-theme]` values.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SidebarTheme {
    #[default]
    Graphite,
    Midnight,
    Arctic,
    Lavender,
    Mint,
    Peach,
    Sky,
    Rose,
    Butter,
    Sage,
    Slate,
    Mauve,
    Sand,
    Mocha,
    Crimson,
    Amber,
    Olive,
    Forest,
    Teal,
    Ocean,
    Indigo,
    Plum,
}

/// A theme name this build doesn't know falls back to Graphite.
fn lenient_sidebar_theme<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SidebarTheme, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// The dark theme's neutral ramp (apps/desktop/src/styles/penguin.css,
/// `[data-dark-shade]`). Light mode ignores it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DarkShade {
    /// True black, the original look.
    #[default]
    Black,
    /// A soft neutral near-black, like macOS.
    Charcoal,
    /// A lighter blue-gray, like GitHub's dark dimmed.
    Dim,
    /// Deep midnight blue.
    Navy,
}

/// A shade this build doesn't know falls back to Black.
fn lenient_dark_shade<'de, D: serde::Deserializer<'de>>(d: D) -> Result<DarkShade, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// The accent (caret, focus ring, unread dot, selection) when it doesn't
/// match the sidebar theme (apps/desktop/src/styles/themes.css).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AccentColor {
    #[default]
    Blue,
    Purple,
    Pink,
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Graphite,
}

/// An accent this build doesn't know falls back to Blue.
fn lenient_accent_color<'de, D: serde::Deserializer<'de>>(d: D) -> Result<AccentColor, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// How round panels, buttons and rows are (apps/desktop/src/styles/themes.css).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CornerStyle {
    #[default]
    Rounded,
    Subtle,
    Square,
}

/// A corner style this build doesn't know falls back to Rounded.
fn lenient_corner_style<'de, D: serde::Deserializer<'de>>(d: D) -> Result<CornerStyle, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// Composer font (Settings → Compose). Wire names are the ids in
/// apps/desktop/src/lib/composeFonts.ts.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ComposeFont {
    #[default]
    Inter,
    Geist,
    IbmPlexSans,
    SourceSerif4,
    Literata,
    IaWriterQuattro,
    AtkinsonHyperlegible,
}

/// A font name this build doesn't know falls back to Inter.
fn lenient_compose_font<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ComposeFont, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

pub const COMPOSE_FONT_SIZE_MIN: u8 = 12;
pub const COMPOSE_FONT_SIZE_MAX: u8 = 20;
pub const COMPOSE_FONT_SIZE_DEFAULT: u8 = 15;
/// Check spelling while typing: on, like Safari and Mail. Also WebKit's
/// starting state on a first launch (src/spelling.rs `prime`).
pub const CHECK_SPELLING_DEFAULT: bool = true;
/// Sidebar text size, in steps from the default (each step ≈ 1px).
pub const SIDEBAR_TEXT_STEP_MIN: i8 = -2;
pub const SIDEBAR_TEXT_STEP_MAX: i8 = 2;

/// An out-of-range or non-numeric size loads as the default.
/// Gmail's default per-user quota is 15,000 units/min, but Penguin paces
/// against a conservative 6,000 until Google approves more for the project.
pub const GMAIL_UNITS_PER_MIN_DEFAULT: u32 = 6_000;
pub const GMAIL_UNITS_PER_MIN_MIN: u32 = 600;
pub const GMAIL_UNITS_PER_MIN_MAX: u32 = 1_000_000;

fn lenient_gmail_units<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (GMAIL_UNITS_PER_MIN_MIN..=GMAIL_UNITS_PER_MIN_MAX).contains(n))
        .unwrap_or(GMAIL_UNITS_PER_MIN_DEFAULT))
}

/// Months of mail downloaded in full (Settings → Sync); 0 = everything.
pub const SYNC_WINDOW_CHOICES: [u32; 6] = penguin_provider::window::WINDOW_MONTH_CHOICES;
pub const SYNC_WINDOW_DEFAULT: u32 = penguin_provider::window::WINDOW_MONTHS_DEFAULT;

fn lenient_sync_window<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| SYNC_WINDOW_CHOICES.contains(n))
        .unwrap_or(SYNC_WINDOW_DEFAULT))
}

/// What happens to mail older than the sync window.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OlderMail {
    /// Headers, labels and snippet only, after the window is complete.
    #[default]
    Headers,
    /// Not downloaded ("Also search Gmail" still finds it).
    None,
    /// Downloaded in full after the window.
    Full,
}

fn lenient_older_mail<'de, D: serde::Deserializer<'de>>(d: D) -> Result<OlderMail, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// A list of account ids; anything that isn't a string is skipped, and a
/// value that isn't a list reads as empty.
fn lenient_account_ids<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// Unfinished setups; entries that don't parse are skipped, and a value that
/// isn't a list reads as empty.
fn lenient_pending_setups<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<PendingSetup>, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default())
}

/// Follow up's wait: an out-of-range or non-numeric value loads as the
/// default (3 days).
fn lenient_follow_up_days<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    use penguin_core::{FOLLOW_UP_DAYS_DEFAULT, FOLLOW_UP_DAYS_MAX, FOLLOW_UP_DAYS_MIN};
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (FOLLOW_UP_DAYS_MIN..=FOLLOW_UP_DAYS_MAX).contains(n))
        .unwrap_or(FOLLOW_UP_DAYS_DEFAULT))
}

fn lenient_compose_font_size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| (COMPOSE_FONT_SIZE_MIN..=COMPOSE_FONT_SIZE_MAX).contains(n))
        .unwrap_or(COMPOSE_FONT_SIZE_DEFAULT))
}

fn lenient_sidebar_text_size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i8, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_i64()
        .and_then(|n| i8::try_from(n).ok())
        .filter(|n| (SIDEBAR_TEXT_STEP_MIN..=SIDEBAR_TEXT_STEP_MAX).contains(n))
        .unwrap_or(0))
}

/// A named group of accounts (a company, a role). The UI can narrow the
/// inbox, search, labels and sync to one profile; the active profile is UI
/// state, not a setting. An account may be in several profiles.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    /// Stable id chosen by the UI: 1–64 of [A-Za-z0-9_-].
    pub id: String,
    pub name: String,
    /// `#rrggbb`, from the account palette.
    pub color: String,
    /// Member account ids (lowercased emails), in the user's order.
    pub account_ids: Vec<String>,
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: ThemeSetting,
    pub density: Density,
    #[serde(deserialize_with = "lenient_list_style")]
    pub list_style: ListStyle,
    /// Split Inbox: `inbox_splits` as tabs above the inbox, plus Other.
    /// Off by default. (The key keeps its old name from when the tabs were
    /// a fixed Important / Other / Newsletters set.)
    pub inbox_tabs: bool,
    /// The Split Inbox's splits, in tab order; a conversation goes to the
    /// first that matches (penguin-core `store_split.rs`). Starts with
    /// Important, Calendar and News.
    #[serde(deserialize_with = "lenient_inbox_splits")]
    pub inbox_splits: Vec<InboxSplit>,
    /// Get to zero: "Archive older than…" for the inbox or a split (⌘K and
    /// the list header). On by default.
    pub get_to_zero: bool,
    /// The inbox-zero screen celebrates (the penguin on its floe, how much
    /// you cleared today). On by default; off shows a plain "No mail".
    pub zero_celebration: bool,
    pub remote_images: RemoteImages,
    /// Lowercased addresses whose remote images load without asking.
    pub trusted_image_senders: Vec<String>,
    /// Remove tracking pixels (known trackers, tiny or hidden images) even
    /// when remote images load. On by default. Off: they are treated like
    /// any other remote image (still found and shown in the privacy row).
    pub block_tracking_pixels: bool,
    /// Remove per-person tracking parameters (`mc_eid`, `fbclid`, …) from
    /// links in received mail (penguin_render::links). Off by default.
    pub strip_link_tracking: bool,
    /// Ask for a read receipt on mail you send (RFC 8098
    /// Disposition-Notification-To; the recipient's app decides whether
    /// one comes back). Off by default. See docs/PRIVACY.md.
    pub request_read_receipts: bool,
    /// Ordered: the first nine get ⌃1–⌃9.
    pub profiles: Vec<Profile>,
    /// Account ids (lowercased emails) left out of the implicit "All
    /// accounts" scope: they stay signed in and syncing, and show their mail
    /// when picked on their own or through a profile that lists them.
    #[serde(deserialize_with = "lenient_account_ids")]
    pub hidden_from_all: Vec<String>,
    /// Account ids (lowercased emails) in the order the user dragged them
    /// into. Every list of accounts (sidebar, switcher, Settings, compose
    /// From, ⌥1–⌥9) follows it; accounts not listed keep their natural
    /// order after the listed ones, so a new account goes last.
    #[serde(deserialize_with = "lenient_account_ids")]
    pub account_order: Vec<String>,
    /// Swipe a list row right / left / far left.
    #[serde(deserialize_with = "lenient_swipe_right")]
    pub swipe_right: SwipeAction,
    #[serde(deserialize_with = "lenient_swipe_left")]
    pub swipe_left: SwipeAction,
    #[serde(deserialize_with = "lenient_swipe_left_long")]
    pub swipe_left_long: SwipeAction,
    /// Sidebar tint; Graphite (neutral) by default.
    #[serde(deserialize_with = "lenient_sidebar_theme")]
    pub sidebar_theme: SidebarTheme,
    /// The accent (caret, focus, unread) follows the sidebar theme. Off by default.
    pub match_accent: bool,
    /// The dark theme's shade; Black by default.
    #[serde(deserialize_with = "lenient_dark_shade")]
    pub dark_shade: DarkShade,
    /// Accent color; Blue by default. `match_accent` overrides it.
    #[serde(deserialize_with = "lenient_accent_color")]
    pub accent_color: AccentColor,
    /// Corner style; Rounded by default.
    #[serde(deserialize_with = "lenient_corner_style")]
    pub corners: CornerStyle,
    /// Experimental: in the dark theme, HTML mail is shown dark (its own
    /// dark styles, or its colors adapted UI-side). Off by default.
    pub dark_email_bodies: bool,
    /// Local MCP server (`penguin-cli mcp`) for AI tools. Off by default.
    pub mcp: McpSettings,
    /// Start in Floe mode (the single-column layout). Off by default.
    pub floe_mode: bool,
    /// Composer font; Inter by default. Local display only, plus a
    /// font-family stack in outgoing HTML.
    #[serde(deserialize_with = "lenient_compose_font")]
    pub compose_font: ComposeFont,
    /// Composer font size in px, 12–20.
    #[serde(deserialize_with = "lenient_compose_font_size")]
    pub compose_font_size: u8,
    /// Sidebar text size in steps from the default, −2…+2 (0 = default).
    #[serde(deserialize_with = "lenient_sidebar_text_size")]
    pub sidebar_text_size: i8,
    /// Follow up lists sent mail with no reply after this many days (1–14).
    #[serde(deserialize_with = "lenient_follow_up_days")]
    pub follow_up_days: u32,
    /// Gmail API quota the sync paces against, per account (units/min).
    /// Applied live; PENGUIN_GMAIL_UNITS_PER_MIN overrides it.
    #[serde(deserialize_with = "lenient_gmail_units")]
    pub gmail_units_per_min: u32,
    /// Composer snippets, inserted by typing ";trigger" or picked with ⌘;. Max 200.
    pub snippets: Vec<Snippet>,
    /// Undo-send window in seconds: 0 (off), 5, 10 (default), 20 or 30.
    #[serde(deserialize_with = "lenient_undo_send_seconds")]
    pub undo_send_seconds: u8,
    /// The hour (5–11, local) the "Tomorrow morning" and "Monday morning"
    /// send-later presets use. Default 8.
    #[serde(deserialize_with = "lenient_send_later_hour")]
    pub send_later_hour: u8,
    /// Instant replies: one-liners offered in the reply composer (⌃1–⌃9)
    /// and the thread's reply box, plus optional on-device suggestions.
    #[serde(deserialize_with = "lenient_instant_replies")]
    pub instant_replies: InstantReplies,
    /// Write with AI in the composer (Apple's on-device model, src/writing/):
    /// draft from an instruction, rewrite a selection. On by default; runs
    /// only when asked, and only on this Mac.
    pub write_with_ai: bool,
    /// Underline misspelled words as you type, in every window (WebKit's
    /// continuous spell checking; src/spelling.rs). On by default; Edit ▸
    /// Spelling and Grammar ▸ Check Spelling While Typing is the same switch.
    pub check_spelling: bool,
    /// Check grammar along with spelling (Edit ▸ Spelling and Grammar ▸
    /// Check Grammar With Spelling). Off by default, as in Safari and Mail.
    pub check_grammar: bool,
    /// Composer signatures (rich text; HTML through the composer allowlist).
    #[serde(deserialize_with = "lenient_signatures")]
    pub signatures: Vec<Signature>,
    /// Account id → its default signature id.
    #[serde(deserialize_with = "lenient_signature_defaults")]
    pub signature_defaults: BTreeMap<String, String>,
    /// When the account's default signature is inserted by itself.
    #[serde(deserialize_with = "lenient_signature_insert")]
    pub signature_insert: SignatureInsert,
    /// Put the "-- " separator line above the signature.
    pub signature_separator: bool,
    /// Replies and forwards go only from the account the mail came to: the
    /// composer won't switch From, and won't send from another account.
    pub lock_reply_account: bool,
    /// Months of mail downloaded in full: 1, 3, 6 (default), 12, 24, or 0
    /// for everything. Applied live; growing it downloads the new band,
    /// shrinking it never deletes (see `free_up_space`).
    #[serde(deserialize_with = "lenient_sync_window")]
    pub sync_window_months: u32,
    /// Mail older than the window: headers only (default), none, or full.
    #[serde(deserialize_with = "lenient_older_mail")]
    pub older_mail: OlderMail,
    /// Key hints (the "E", "⌘K" key caps) shown across the UI. Off by
    /// default, which hides them everywhere but the "?" cheat sheet; on
    /// shows them on buttons, rows and footers. The shortcuts themselves
    /// work either way.
    pub show_shortcut_hints: bool,
    /// Shortcut coach: after a mouse action that has a key, show that key
    /// for a moment (rate-limited; keys you use stop being hinted). On by
    /// default. UI only (src/app/ShortcutCoach.tsx).
    pub shortcut_coach: bool,
    /// The Unsubscribe button in the thread view's privacy row (and its
    /// menu item and ⌘U). On by default.
    pub unsubscribe_button: bool,
    /// Search by meaning (Settings → Search): download the embedding model
    /// once and index mail in the background (src/semantic/). On by
    /// default; off frees the model and the vectors from memory and keeps
    /// semantic.db on disk.
    pub semantic_search: bool,
    /// Thread summaries with Apple's on-device model (src/summary/): the
    /// Summarize action and cached summaries in the thread view. On by
    /// default; nothing runs until you ask, and only on this Mac.
    pub summaries: bool,
    /// Ask reads a question its grammar can't with Apple's on-device model
    /// (src/ask.rs, `ask_understand`): the model only turns the question
    /// into a query, which is checked and answered from local mail. On by
    /// default; runs only when Apple Intelligence is available.
    pub ask_with_ai: bool,
    /// Sender photos and logos (src/avatars/): which sources may be used.
    pub sender_photos: SenderPhotos,
    /// Where sender photos appear (Settings → General → Appearance).
    #[serde(deserialize_with = "lenient_avatar_placement")]
    pub avatar_placement: AvatarPlacement,
    /// "You" (Settings → You): name, photo, title shown for yourself.
    pub me: Me,
    /// Google Calendar (src/calendar/): sync window and the status-bar chip.
    pub calendar: CalendarSettings,
    /// New-mail notifications (src/notify.rs). Off by default.
    pub notifications: NotificationSettings,
    /// Add account setups started but not finished (no secrets): shown in
    /// Settings → Accounts and on the Add account start screen with Resume
    /// and Remove. An entry goes away when its account is added (see
    /// `for_accounts`) or when the user removes it.
    #[serde(deserialize_with = "lenient_pending_setups")]
    pub pending_setups: Vec<PendingSetup>,
    /// Smart views in the sidebar (Receipts, Travel, … and pinned saved
    /// searches; penguin-core `store_smart.rs`). All off by default.
    #[serde(deserialize_with = "lenient_smart_views")]
    pub smart_views: SmartViewSettings,
    /// The Welcome setup (features/welcome) was finished or skipped. False
    /// on a new install: it shows once after the first account is added,
    /// and the search model isn't downloaded before it (src/semantic/). A
    /// file from before this setting, on an install with accounts, counts
    /// as done (`SettingsState::settle_welcome`). docs/ONBOARDING.md.
    pub welcome_completed: bool,
}

/// One tab of the Split Inbox.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct InboxSplit {
    /// 1–64 of [A-Za-z0-9_-], chosen by the UI.
    pub id: String,
    /// Trimmed, ≤30 chars; blank = the query.
    pub name: String,
    /// What it holds, in the search language (≤500 chars).
    pub query: String,
    /// Leave the tab out while it has nothing in it.
    pub hide_when_empty: bool,
}

pub const MAX_INBOX_SPLITS: usize = 12;
const MAX_INBOX_SPLIT_NAME: usize = 30;

/// The splits a new Split Inbox starts with (the UI's presets of the same
/// names): Important (Gmail's own marker, minus bulk mail and invitations),
/// Calendar and News. Other is always there.
fn default_inbox_splits() -> Vec<InboxSplit> {
    let split = |id: &str, name: &str, query: &str| InboxSplit {
        id: id.into(),
        name: name.into(),
        query: query.into(),
        hide_when_empty: false,
    };
    vec![
        split(
            "important",
            "Important",
            "is:important -is:newsletter -has:invite",
        ),
        split(
            "calendar",
            "Calendar",
            "has:invite OR from:calendar-notification@google.com OR from:@calendly.com",
        ),
        split("news", "News", "is:newsletter"),
    ]
}

/// The splits list; a malformed one loads as the defaults.
fn lenient_inbox_splits<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<InboxSplit>, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_else(|_| default_inbox_splits()))
}

/// Valid ids, deduped, names and queries trimmed and bounded, no empty
/// query, at most `MAX_INBOX_SPLITS`.
fn normalize_inbox_splits(splits: Vec<InboxSplit>) -> Vec<InboxSplit> {
    let mut out: Vec<InboxSplit> = Vec::new();
    for mut sp in splits {
        let id_ok = (1..=64).contains(&sp.id.len())
            && sp
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let query: String = truncate_chars(
            &sp.query.split_whitespace().collect::<Vec<_>>().join(" "),
            MAX_CUSTOM_VIEW_QUERY,
        );
        if !id_ok || query.is_empty() || out.iter().any(|x| x.id == sp.id) {
            continue;
        }
        let name = sp.name.split_whitespace().collect::<Vec<_>>().join(" ");
        sp.name = truncate_chars(
            if name.is_empty() { &query } else { &name },
            MAX_INBOX_SPLIT_NAME,
        );
        sp.query = query;
        out.push(sp);
        if out.len() == MAX_INBOX_SPLITS {
            break;
        }
    }
    out
}

/// Which smart views the sidebar shows, in what order, and with a count.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartViewSettings {
    /// Shown in the sidebar, in order: built-in view ids
    /// (`penguin_core::SMART_VIEWS`) and pinned searches as `custom:<id>`.
    /// A pinned search is always shown (removing it unpins it).
    pub shown: Vec<String>,
    /// Views whose sidebar row shows a count; off by default.
    pub counts: Vec<String>,
    /// Saved searches pinned to the sidebar.
    pub custom: Vec<CustomView>,
}

/// A saved search pinned as a sidebar view.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomView {
    /// 1–64 of [A-Za-z0-9_-], chosen by the UI.
    pub id: String,
    /// Trimmed, ≤40 chars; blank = the query.
    pub name: String,
    /// The search, in the search language (≤500 chars).
    pub query: String,
}

pub const MAX_CUSTOM_VIEWS: usize = 30;
const MAX_CUSTOM_VIEW_NAME: usize = 40;
const MAX_CUSTOM_VIEW_QUERY: usize = 500;

/// The smart views section; a malformed one loads as all off.
fn lenient_smart_views<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<SmartViewSettings, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// Valid, deduped, bounded; every pinned search in `shown`; ids that name
/// nothing dropped.
fn normalize_smart_views(v: SmartViewSettings) -> SmartViewSettings {
    let mut custom: Vec<CustomView> = Vec::new();
    for mut c in v.custom {
        let id_ok = !c.id.is_empty()
            && c.id.len() <= 64
            && c.id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
        let query: String = c
            .query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(MAX_CUSTOM_VIEW_QUERY)
            .collect();
        if !id_ok || query.is_empty() || custom.iter().any(|x| x.id == c.id) {
            continue;
        }
        let name: String = c.name.split_whitespace().collect::<Vec<_>>().join(" ");
        let name = if name.is_empty() { query.clone() } else { name };
        c.name = name.chars().take(MAX_CUSTOM_VIEW_NAME).collect();
        c.query = query;
        custom.push(c);
        if custom.len() == MAX_CUSTOM_VIEWS {
            break;
        }
    }
    let known = |id: &str| match id.strip_prefix("custom:") {
        Some(c) => custom.iter().any(|x| x.id == c),
        None => penguin_core::SMART_VIEWS.contains(&id),
    };
    let mut shown: Vec<String> = Vec::new();
    for id in v.shown {
        if known(&id) && !shown.contains(&id) {
            shown.push(id);
        }
    }
    for c in &custom {
        let id = format!("custom:{}", c.id);
        if !shown.contains(&id) {
            shown.push(id);
        }
    }
    let mut counts: Vec<String> = Vec::new();
    for id in v.counts {
        if known(&id) && !counts.contains(&id) {
            counts.push(id);
        }
    }
    SmartViewSettings {
        shown,
        counts,
        custom,
    }
}

/// One unfinished Add account setup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingSetup {
    /// The address being added (lowercased).
    pub email: String,
    /// The provider path it's on.
    pub kind: crate::providers::SetupKind,
    /// The Add account step reached (a step id of the UI's flow).
    pub step: String,
    /// Unix ms when the setup started.
    pub started_at: i64,
    /// Why the last sign-in or connect attempt failed; null when none has.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Settings → Calendar. Connecting is per account (at sign-in, or later with
/// `connect_calendar`); these apply to every connected account.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarSettings {
    /// Months of past events kept: 1–60, default 24.
    pub past_months: u32,
    /// Months of upcoming events: 1–36, default 12.
    pub future_months: u32,
    /// "Next up: 3:00 PM Weekly sync (in 25 min)" in the status bar.
    pub next_up: bool,
    /// Adding or reconnecting an account also asks for read-only calendar
    /// access in the same Google consent. Default true. Off is for a
    /// Workspace whose admin allows Gmail but blocks Calendar, where asking
    /// for both makes Google refuse the whole sign-in.
    pub connect_on_sign_in: bool,
}

pub const CALENDAR_PAST_MONTHS: (u32, u32) = (1, 60);
pub const CALENDAR_FUTURE_MONTHS: (u32, u32) = (1, 36);

impl Default for CalendarSettings {
    fn default() -> Self {
        CalendarSettings {
            past_months: 24,
            future_months: 12,
            next_up: true,
            connect_on_sign_in: true,
        }
    }
}

/// Settings → General → Notifications: macOS notifications for new mail
/// (incremental sync only, never backfill). Sender and subject only.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationSettings {
    /// "Notify me about new mail". Off by default.
    pub enabled: bool,
    /// Per-account switches (account id → on). An account not listed
    /// follows the default: on, except accounts hidden from All Inboxes.
    #[serde(deserialize_with = "lenient_bool_map")]
    pub accounts: BTreeMap<String, bool>,
    /// Only mail from people I've emailed before (from any account).
    pub known_senders_only: bool,
}

/// A map of booleans; entries that aren't booleans are skipped, and a value
/// that isn't an object reads as empty.
fn lenient_bool_map<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, bool>, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
                .collect()
        })
        .unwrap_or_default())
}

impl Settings {
    /// Whether new mail in `account_id` may notify (the master switch, then
    /// the account's own switch or its default).
    pub fn notifies_for(&self, account_id: &str) -> bool {
        let n = &self.notifications;
        n.enabled
            && n.accounts
                .get(account_id)
                .copied()
                .unwrap_or_else(|| !self.hidden_from_all.iter().any(|h| h == account_id))
    }
}

/// "Sender photos appear": in the message list, in message headers, both
/// (default) or nowhere. Off also stops every avatar lookup and fetch.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AvatarPlacement {
    List,
    Message,
    #[default]
    Both,
    Off,
}

/// An unknown placement falls back to Both.
fn lenient_avatar_placement<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<AvatarPlacement, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// Settings → Privacy → Sender photos: which sources may be used. Honored
/// by the avatar service.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SenderPhotos {
    /// Google contact photos (People API; needs "Connect contact photos"
    /// per account). Off until connected.
    pub contacts: bool,
    /// BIMI brand logos for authenticated senders. On by default.
    pub bimi: bool,
    /// The sender domain's own icon, fetched from that domain. On by default.
    pub favicons: bool,
    /// Gravatar by address hash; tells Gravatar who emails you. Off by default.
    pub gravatar: bool,
}

impl Default for SenderPhotos {
    fn default() -> Self {
        SenderPhotos {
            contacts: false,
            bimi: true,
            favicons: true,
            gravatar: false,
        }
    }
}

impl Settings {
    /// What the avatar service may do.
    pub fn avatar_prefs(&self) -> crate::avatars::Prefs {
        let p = &self.sender_photos;
        crate::avatars::Prefs {
            show: self.avatar_placement != AvatarPlacement::Off,
            contacts: p.contacts,
            bimi: p.bimi,
            favicons: p.favicons,
            gravatar: p.gravatar,
        }
    }
}

/// A composer snippet. The body may use {first_name}, {last_name},
/// {full_name}, {company}, {sender_name}, {my_name}, {my_first_name},
/// {date} and {cursor}; the UI fills them in (features/compose/snippets.ts).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Snippet {
    /// Stable id chosen by the UI: 1–64 of [A-Za-z0-9_-].
    pub id: String,
    /// Typed after ";": 1–32 of [a-z0-9_-], unique.
    pub trigger: String,
    pub title: String,
    pub body: String,
    pub uses: u32,
    /// Subject it fills in when the message has none yet; empty = none. ≤ 200 chars.
    pub subject: String,
    /// Addresses ("Name <a@b.example>" or a bare address) it adds to Cc / Bcc. ≤ 20 each.
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    /// Files it attaches (kept on this Mac by `save_snippet_file`). ≤ 10.
    pub attachments: Vec<SnippetFile>,
}

/// A file kept with a snippet, stored at `<data dir>/snippets/<id>`
/// (src/snippet_files.rs); `id` is the sha256 of its bytes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SnippetFile {
    /// 64 lowercase hex.
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

/// Instant replies (Settings → Compose).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct InstantReplies {
    /// Offer the one-liners. On by default.
    pub enabled: bool,
    /// The one-liners, in order (⌃1…⌃9). ≤ 9, each ≤ 200 chars.
    pub replies: Vec<String>,
    /// Also suggest up to three replies to the conversation with Apple's
    /// on-device model when a reply opens. Off by default.
    pub ai_suggestions: bool,
}

impl Default for InstantReplies {
    fn default() -> Self {
        InstantReplies {
            enabled: true,
            replies: vec![
                "Sounds good, thanks!".into(),
                "Thanks, got it.".into(),
                "Let me check and get back to you.".into(),
            ],
            ai_suggestions: false,
        }
    }
}

pub const MAX_INSTANT_REPLIES: usize = 9;
const MAX_INSTANT_REPLY: usize = 200;

fn lenient_instant_replies<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<InstantReplies, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// Trimmed, non-empty, deduped, ≤ 200 chars each, at most nine.
fn normalize_instant_replies(mut r: InstantReplies) -> InstantReplies {
    let mut seen = HashSet::new();
    r.replies = r
        .replies
        .into_iter()
        .map(|t| truncate_chars(t.trim(), MAX_INSTANT_REPLY))
        .filter(|t| !t.is_empty() && seen.insert(t.clone()))
        .take(MAX_INSTANT_REPLIES)
        .collect();
    r
}

/// A composer signature. `html` only ever holds what
/// `penguin_render::sanitize_compose_html` returns.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Signature {
    /// Stable id chosen by the UI: 1–64 of [A-Za-z0-9_-].
    pub id: String,
    pub name: String,
    pub html: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SignatureInsert {
    pub new_messages: bool,
    pub replies: bool,
    pub forwards: bool,
}

impl Default for SignatureInsert {
    fn default() -> Self {
        SignatureInsert {
            new_messages: true,
            replies: true,
            forwards: true,
        }
    }
}

pub const MAX_SIGNATURES: usize = 50;

// A hand-edited file with one bad signature entry keeps the rest (and the
// rest of the settings): malformed items are skipped, not fatal.
fn lenient_signatures<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Signature>, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(match raw {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        _ => Vec::new(),
    })
}

fn lenient_signature_defaults<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(match raw {
        serde_json::Value::Object(map) => map
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|id| (k, id.to_string())))
            .collect(),
        _ => BTreeMap::new(),
    })
}

fn lenient_signature_insert<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<SignatureInsert, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}
const MAX_SIGNATURE_NAME: usize = 60;
const MAX_SIGNATURE_HTML: usize = 20_000;

/// Drop signatures with a bad or duplicate id or oversized HTML, sanitize
/// the HTML, and keep only defaults that point at a signature.
fn normalize_signatures(
    signatures: Vec<Signature>,
    defaults: BTreeMap<String, String>,
) -> (Vec<Signature>, BTreeMap<String, String>) {
    let mut seen = HashSet::new();
    let signatures: Vec<Signature> = signatures
        .into_iter()
        .filter_map(|mut s| {
            let id_ok = (1..=64).contains(&s.id.len())
                && s.id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !id_ok || s.html.len() > MAX_SIGNATURE_HTML * 4 || !seen.insert(s.id.clone()) {
                return None;
            }
            s.name = truncate_chars(s.name.trim(), MAX_SIGNATURE_NAME);
            if s.name.is_empty() {
                s.name = "Signature".into();
            }
            s.html = penguin_render::sanitize_compose_html(&s.html);
            (s.html.chars().count() <= MAX_SIGNATURE_HTML).then_some(s)
        })
        .take(MAX_SIGNATURES)
        .collect();
    let defaults = defaults
        .into_iter()
        .map(|(a, id)| (a.trim().to_lowercase(), id))
        .filter(|(a, id)| !a.is_empty() && a.len() <= 320 && signatures.iter().any(|s| &s.id == id))
        .collect();
    (signatures, defaults)
}

pub const MAX_SNIPPETS: usize = 200;
const MAX_SNIPPET_TITLE: usize = 80;
const MAX_SNIPPET_BODY: usize = 10_000;
pub const UNDO_SEND_CHOICES: [u8; 5] = [0, 5, 10, 20, 30];
pub const UNDO_SEND_DEFAULT: u8 = 10;
pub const SEND_LATER_HOURS: std::ops::RangeInclusive<u8> = 5..=11;
pub const SEND_LATER_HOUR_DEFAULT: u8 = 8;
const MAX_SNIPPET_SUBJECT: usize = 200;
const MAX_SNIPPET_ADDRESSES: usize = 20;
const MAX_SNIPPET_FILES: usize = 10;

fn lenient_send_later_hour<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| SEND_LATER_HOURS.contains(n))
        .unwrap_or(SEND_LATER_HOUR_DEFAULT))
}

/// Snippet addresses: trimmed, non-empty, ≤ 320 chars, deduped (case-blind), ≤ 20.
fn normalize_snippet_addresses(list: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    list.into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty() && a.len() <= 320 && seen.insert(a.to_lowercase()))
        .take(MAX_SNIPPET_ADDRESSES)
        .collect()
}

/// Snippet files: a sha256 id, a display name, deduped, ≤ 10.
fn normalize_snippet_files(list: Vec<SnippetFile>) -> Vec<SnippetFile> {
    let mut seen = HashSet::new();
    list.into_iter()
        .filter_map(|mut f| {
            let id_ok = f.id.len() == 64
                && f.id
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
            if !id_ok || !seen.insert(f.id.clone()) {
                return None;
            }
            f.filename = truncate_chars(f.filename.trim(), 200);
            if f.filename.is_empty() {
                f.filename = "Attachment".into();
            }
            f.mime_type = truncate_chars(f.mime_type.trim(), 100);
            if f.mime_type.is_empty() {
                f.mime_type = "application/octet-stream".into();
            }
            Some(f)
        })
        .take(MAX_SNIPPET_FILES)
        .collect()
}

fn lenient_undo_send_seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(raw
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| UNDO_SEND_CHOICES.contains(n))
        .unwrap_or(UNDO_SEND_DEFAULT))
}

/// Fictional starters for a fresh install.
fn default_snippets() -> Vec<Snippet> {
    vec![
        Snippet {
            id: "thanks".into(),
            trigger: "thx".into(),
            title: "Thanks + next steps".into(),
            body: "Thanks, {first_name}. Next steps on my side: {cursor}. I'll follow up by {day}.\n\n{my_name}".into(),
            ..Default::default()
        },
        Snippet {
            id: "review".into(),
            trigger: "review".into(),
            title: "Review checklist".into(),
            body: "could you review the deck ahead of time and flag anything blocking by {day before}? That keeps the session focused on decisions.".into(),
            ..Default::default()
        },
    ]
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Drop snippets with a bad id or trigger, trim sizes, keep the first of any
/// duplicate trigger, and cap the list.
fn normalize_snippets(snippets: Vec<Snippet>) -> Vec<Snippet> {
    let mut seen_ids = HashSet::new();
    let mut seen_triggers = HashSet::new();
    snippets
        .into_iter()
        .filter_map(|mut s| {
            s.trigger = s.trigger.trim().to_lowercase();
            let id_ok = (1..=64).contains(&s.id.len())
                && s.id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            let trigger_ok = (1..=32).contains(&s.trigger.len())
                && s.trigger
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            if !id_ok
                || !trigger_ok
                || !seen_ids.insert(s.id.clone())
                || !seen_triggers.insert(s.trigger.clone())
            {
                return None;
            }
            s.title = truncate_chars(s.title.trim(), MAX_SNIPPET_TITLE);
            s.body = truncate_chars(&s.body, MAX_SNIPPET_BODY);
            s.subject = truncate_chars(s.subject.trim(), MAX_SNIPPET_SUBJECT);
            s.cc = normalize_snippet_addresses(std::mem::take(&mut s.cc));
            s.bcc = normalize_snippet_addresses(std::mem::take(&mut s.bcc));
            s.attachments = normalize_snippet_files(std::mem::take(&mut s.attachments));
            Some(s)
        })
        .take(MAX_SNIPPETS)
        .collect()
}

/// "You": the user's own identity across every account (Settings → You).
/// Display-only: outgoing mail keeps each Google account's own display name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Me {
    pub name: String,
    pub title: String,
    pub company: String,
    /// Name to sign off with ("Sam"); empty = the first word of `name`.
    pub sign_off: String,
    /// Id (sha256 hex) of the photo in `<data dir>/me/`. Set only by the
    /// `set_me_photo*` / `clear_me_photo` commands, never by a patch.
    pub photo: Option<String>,
}

/// `update_settings({me})`: the text fields; the photo has its own commands.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MePatch {
    pub name: Option<String>,
    pub title: Option<String>,
    pub company: Option<String>,
    pub sign_off: Option<String>,
}

pub const MAX_ME_FIELD: usize = 80;

/// Trim, collapse inner whitespace (no newlines in a name) and bound.
fn me_field(s: &str) -> String {
    truncate_chars(
        &s.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_ME_FIELD,
    )
}

fn is_photo_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Settings → Developer → "Agents (CLI and MCP)": what `penguin-cli` and its
/// MCP server may do. Ordered: each level includes the ones below it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum AgentAccess {
    /// No MCP server; the CLI can't draft or send.
    #[default]
    Off,
    /// Search and read the local index (the MCP server's read-only tools).
    Read,
    /// Read, plus organize mail (archive, labels, snooze, Trash… all
    /// reversible) and create / update / list / delete its own drafts.
    /// Stored as `draft` (the name from before organizing existed).
    Draft,
    /// Read, organize and draft, plus send (through the outbox, after a
    /// delay the user can cancel). Only `SettingsState::grant_agent_send`
    /// sets it.
    Send,
}

impl AgentAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentAccess::Off => "off",
            AgentAccess::Read => "read",
            AgentAccess::Draft => "draft",
            AgentAccess::Send => "send",
        }
    }

    /// What Settings calls the level.
    pub fn label(self) -> &'static str {
        match self {
            AgentAccess::Off => "Off",
            AgentAccess::Read => "Read only",
            AgentAccess::Draft => "Read, organize and draft",
            AgentAccess::Send => "Read, organize, draft and send",
        }
    }
}

/// Seconds an agent's send waits in the outbox before it goes (the user can
/// cancel it meanwhile). Anything else loads as the default.
pub const AGENT_SEND_DELAYS: [u32; 4] = [10, 30, 60, 300];
pub const AGENT_SEND_DELAY_DEFAULT: u32 = 60;

/// `penguin-cli` and its MCP server (`agents` in the UI). Stored under
/// `mcp` for compatibility: older builds wrote `{"enabled": bool}`, which
/// loads as Read (true) or Off (false). An unknown `access` value loads as
/// Off. `enabled` is still written (true for any level above Off) so an
/// older build reading this file keeps the MCP server's on/off state.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    pub access: AgentAccess,
    /// Derived from `access`; kept in the file for older builds.
    pub enabled: bool,
    /// An agent's send waits this long in the outbox (AGENT_SEND_DELAYS).
    pub send_delay_seconds: u32,
    /// Agents may only send to addresses I've sent mail to before (from any
    /// account) or to my own accounts. On by default.
    pub send_known_only: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        McpSettings::with_access(AgentAccess::Off)
    }
}

impl McpSettings {
    pub fn with_access(access: AgentAccess) -> McpSettings {
        McpSettings {
            access,
            enabled: access > AgentAccess::Off,
            send_delay_seconds: AGENT_SEND_DELAY_DEFAULT,
            send_known_only: true,
        }
    }

    fn set_access(&mut self, access: AgentAccess) {
        self.access = access;
        self.enabled = access > AgentAccess::Off;
    }
}

impl<'de> Deserialize<'de> for McpSettings {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(d)?;
        let obj = raw.as_object();
        let field = |k: &str| obj.and_then(|o| o.get(k));
        let access = match field("access") {
            // Present: it decides, and anything unrecognized is Off.
            Some(v) => serde_json::from_value(v.clone()).unwrap_or(AgentAccess::Off),
            // A file from before levels existed.
            None => match field("enabled").and_then(|v| v.as_bool()) {
                Some(true) => AgentAccess::Read,
                _ => AgentAccess::Off,
            },
        };
        let mut out = McpSettings::with_access(access);
        if let Some(n) = field("sendDelaySeconds").and_then(|v| v.as_u64()) {
            if let Some(&d) = AGENT_SEND_DELAYS.iter().find(|&&d| u64::from(d) == n) {
                out.send_delay_seconds = d;
            }
        }
        if let Some(b) = field("sendKnownOnly").and_then(|v| v.as_bool()) {
            out.send_known_only = b;
        }
        Ok(out)
    }
}

/// `update_settings({mcp})`: omitted fields are unchanged; unknown keys are
/// ignored (never granted). `access: "send"` is refused here: raising the
/// level to send goes only through `enable_agent_send`, after the
/// confirmation. Lowering to any level works. `enabled` is the old switch
/// (true = at least Read, false = Off).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPatch {
    pub access: Option<AgentAccess>,
    pub enabled: Option<bool>,
    pub send_delay_seconds: Option<u32>,
    pub send_known_only: Option<bool>,
}

impl McpPatch {
    /// Whether applying this would raise the level to Send.
    pub fn raises_to_send(&self, current: AgentAccess) -> bool {
        self.access == Some(AgentAccess::Send) && current != AgentAccess::Send
    }
}

// Manual: the swipe actions have different defaults per direction.
impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeSetting::default(),
            density: Density::default(),
            list_style: ListStyle::default(),
            inbox_tabs: false,
            inbox_splits: default_inbox_splits(),
            get_to_zero: true,
            zero_celebration: true,
            remote_images: RemoteImages::default(),
            trusted_image_senders: Vec::new(),
            block_tracking_pixels: true,
            strip_link_tracking: false,
            request_read_receipts: false,
            profiles: Vec::new(),
            hidden_from_all: Vec::new(),
            account_order: Vec::new(),
            swipe_right: SwipeAction::ToggleRead,
            swipe_left: SwipeAction::Archive,
            swipe_left_long: SwipeAction::Trash,
            sidebar_theme: SidebarTheme::default(),
            match_accent: false,
            dark_shade: DarkShade::default(),
            accent_color: AccentColor::default(),
            corners: CornerStyle::default(),
            dark_email_bodies: false,
            mcp: McpSettings::default(),
            floe_mode: false,
            compose_font: ComposeFont::default(),
            compose_font_size: COMPOSE_FONT_SIZE_DEFAULT,
            sidebar_text_size: 0,
            follow_up_days: penguin_core::FOLLOW_UP_DAYS_DEFAULT,
            gmail_units_per_min: GMAIL_UNITS_PER_MIN_DEFAULT,
            snippets: default_snippets(),
            undo_send_seconds: UNDO_SEND_DEFAULT,
            send_later_hour: SEND_LATER_HOUR_DEFAULT,
            instant_replies: InstantReplies::default(),
            write_with_ai: true,
            check_spelling: CHECK_SPELLING_DEFAULT,
            check_grammar: false,
            signatures: Vec::new(),
            signature_defaults: BTreeMap::new(),
            signature_insert: SignatureInsert::default(),
            signature_separator: false,
            lock_reply_account: true,
            sync_window_months: SYNC_WINDOW_DEFAULT,
            older_mail: OlderMail::default(),
            show_shortcut_hints: false,
            shortcut_coach: true,
            unsubscribe_button: true,
            semantic_search: true,
            summaries: true,
            ask_with_ai: true,
            sender_photos: SenderPhotos::default(),
            avatar_placement: AvatarPlacement::default(),
            me: Me::default(),
            calendar: CalendarSettings::default(),
            notifications: NotificationSettings::default(),
            pending_setups: Vec::new(),
            smart_views: SmartViewSettings::default(),
            welcome_completed: false,
        }
    }
}

/// `update_settings` argument: omitted fields are left unchanged.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub theme: Option<ThemeSetting>,
    pub density: Option<Density>,
    pub list_style: Option<ListStyle>,
    pub inbox_tabs: Option<bool>,
    /// Replaces the whole list (order included); normalized.
    pub inbox_splits: Option<Vec<InboxSplit>>,
    pub get_to_zero: Option<bool>,
    pub zero_celebration: Option<bool>,
    pub remote_images: Option<RemoteImages>,
    pub trusted_image_senders: Option<Vec<String>>,
    pub block_tracking_pixels: Option<bool>,
    pub strip_link_tracking: Option<bool>,
    pub request_read_receipts: Option<bool>,
    /// Replaces the whole list (order included).
    pub profiles: Option<Vec<Profile>>,
    /// Replaces the whole list.
    pub hidden_from_all: Option<Vec<String>>,
    /// Replaces the whole list.
    pub account_order: Option<Vec<String>>,
    pub swipe_right: Option<SwipeAction>,
    pub swipe_left: Option<SwipeAction>,
    pub swipe_left_long: Option<SwipeAction>,
    pub sidebar_theme: Option<SidebarTheme>,
    pub match_accent: Option<bool>,
    pub dark_shade: Option<DarkShade>,
    pub accent_color: Option<AccentColor>,
    pub corners: Option<CornerStyle>,
    pub dark_email_bodies: Option<bool>,
    /// Merged field by field (see McpPatch); never raises the level to send.
    pub mcp: Option<McpPatch>,
    pub floe_mode: Option<bool>,
    pub compose_font: Option<ComposeFont>,
    /// Clamped to 12–20.
    pub compose_font_size: Option<u8>,
    pub sidebar_text_size: Option<i8>,
    /// Clamped to 1–14.
    pub follow_up_days: Option<u32>,
    /// Clamped to 600–1,000,000.
    pub gmail_units_per_min: Option<u32>,
    /// Replaces the whole list (order included).
    pub snippets: Option<Vec<Snippet>>,
    /// 0 (off), 5, 10, 20 or 30; anything else is ignored.
    pub undo_send_seconds: Option<u8>,
    /// 5–11; anything else is ignored.
    pub send_later_hour: Option<u8>,
    /// Replaces the whole section; the one-liners are normalized.
    pub instant_replies: Option<InstantReplies>,
    pub write_with_ai: Option<bool>,
    pub check_spelling: Option<bool>,
    pub check_grammar: Option<bool>,
    /// Replaces the whole list (order included); HTML is re-sanitized.
    pub signatures: Option<Vec<Signature>>,
    /// Replaces the whole map.
    pub signature_defaults: Option<BTreeMap<String, String>>,
    pub signature_insert: Option<SignatureInsert>,
    pub signature_separator: Option<bool>,
    pub lock_reply_account: Option<bool>,
    /// 1, 3, 6, 12, 24 or 0 (everything); anything else is ignored.
    pub sync_window_months: Option<u32>,
    pub older_mail: Option<OlderMail>,
    pub show_shortcut_hints: Option<bool>,
    pub shortcut_coach: Option<bool>,
    pub unsubscribe_button: Option<bool>,
    pub semantic_search: Option<bool>,
    pub summaries: Option<bool>,
    pub ask_with_ai: Option<bool>,
    /// Replaces the whole section.
    pub sender_photos: Option<SenderPhotos>,
    pub avatar_placement: Option<AvatarPlacement>,
    /// Merged field by field; the photo is not patchable.
    pub me: Option<MePatch>,
    /// Replaces the whole section; months are clamped.
    pub calendar: Option<CalendarSettings>,
    /// Replaces the whole section; account ids are lowercased, deduped, ≤100.
    pub notifications: Option<NotificationSettings>,
    /// Replaces the whole list (one entry per address, the last one wins).
    pub pending_setups: Option<Vec<PendingSetup>>,
    /// Replaces the whole section; normalized (unknown ids dropped).
    pub smart_views: Option<SmartViewSettings>,
    pub welcome_completed: Option<bool>,
}

impl Settings {
    /// The `mcp` part of a patch. A request to raise the level to Send is
    /// ignored here (`update_settings` rejects it before this runs; this is
    /// the second line): only `SettingsState::grant_agent_send` sets it.
    fn apply_mcp(&mut self, p: McpPatch) {
        let current = self.mcp.access;
        let wanted = match (p.access, p.enabled) {
            (Some(a), _) => Some(a),
            (None, Some(true)) if current == AgentAccess::Off => Some(AgentAccess::Read),
            (None, Some(false)) => Some(AgentAccess::Off),
            _ => None,
        };
        if let Some(a) = wanted {
            if a != AgentAccess::Send || current == AgentAccess::Send {
                self.mcp.set_access(a);
            }
        }
        if let Some(d) = p.send_delay_seconds {
            if AGENT_SEND_DELAYS.contains(&d) {
                self.mcp.send_delay_seconds = d;
            }
        }
        if let Some(b) = p.send_known_only {
            self.mcp.send_known_only = b;
        }
    }

    pub fn apply(&mut self, patch: SettingsPatch) {
        if let Some(v) = patch.theme {
            self.theme = v;
        }
        if let Some(v) = patch.density {
            self.density = v;
        }
        if let Some(v) = patch.list_style {
            self.list_style = v;
        }
        if let Some(v) = patch.inbox_tabs {
            self.inbox_tabs = v;
        }
        if let Some(v) = patch.inbox_splits {
            self.inbox_splits = v;
        }
        if let Some(v) = patch.get_to_zero {
            self.get_to_zero = v;
        }
        if let Some(v) = patch.zero_celebration {
            self.zero_celebration = v;
        }
        if let Some(v) = patch.remote_images {
            self.remote_images = v;
        }
        if let Some(v) = patch.trusted_image_senders {
            self.trusted_image_senders = v;
        }
        if let Some(v) = patch.block_tracking_pixels {
            self.block_tracking_pixels = v;
        }
        if let Some(v) = patch.strip_link_tracking {
            self.strip_link_tracking = v;
        }
        if let Some(v) = patch.request_read_receipts {
            self.request_read_receipts = v;
        }
        if let Some(v) = patch.profiles {
            self.profiles = v;
        }
        if let Some(v) = patch.hidden_from_all {
            self.hidden_from_all = v;
        }
        if let Some(v) = patch.account_order {
            self.account_order = v;
        }
        if let Some(v) = patch.pending_setups {
            self.pending_setups = v;
        }
        if let Some(v) = patch.swipe_right {
            self.swipe_right = v;
        }
        if let Some(v) = patch.swipe_left {
            self.swipe_left = v;
        }
        if let Some(v) = patch.swipe_left_long {
            self.swipe_left_long = v;
        }
        if let Some(v) = patch.sidebar_theme {
            self.sidebar_theme = v;
        }
        if let Some(v) = patch.match_accent {
            self.match_accent = v;
        }
        if let Some(v) = patch.dark_shade {
            self.dark_shade = v;
        }
        if let Some(v) = patch.accent_color {
            self.accent_color = v;
        }
        if let Some(v) = patch.corners {
            self.corners = v;
        }
        if let Some(v) = patch.dark_email_bodies {
            self.dark_email_bodies = v;
        }
        if let Some(v) = patch.mcp {
            self.apply_mcp(v);
        }
        if let Some(v) = patch.floe_mode {
            self.floe_mode = v;
        }
        if let Some(v) = patch.compose_font {
            self.compose_font = v;
        }
        if let Some(v) = patch.compose_font_size {
            self.compose_font_size = v.clamp(COMPOSE_FONT_SIZE_MIN, COMPOSE_FONT_SIZE_MAX);
        }
        if let Some(v) = patch.sidebar_text_size {
            self.sidebar_text_size = v.clamp(SIDEBAR_TEXT_STEP_MIN, SIDEBAR_TEXT_STEP_MAX);
        }
        if let Some(v) = patch.follow_up_days {
            self.follow_up_days = v.clamp(
                penguin_core::FOLLOW_UP_DAYS_MIN,
                penguin_core::FOLLOW_UP_DAYS_MAX,
            );
        }
        if let Some(v) = patch.gmail_units_per_min {
            self.gmail_units_per_min = v.clamp(GMAIL_UNITS_PER_MIN_MIN, GMAIL_UNITS_PER_MIN_MAX);
        }
        if let Some(v) = patch.snippets {
            self.snippets = v;
        }
        if let Some(v) = patch.signatures {
            self.signatures = v;
        }
        if let Some(v) = patch.signature_defaults {
            self.signature_defaults = v;
        }
        if let Some(v) = patch.signature_insert {
            self.signature_insert = v;
        }
        if let Some(v) = patch.signature_separator {
            self.signature_separator = v;
        }
        if let Some(v) = patch.lock_reply_account {
            self.lock_reply_account = v;
        }
        if let Some(v) = patch
            .undo_send_seconds
            .filter(|v| UNDO_SEND_CHOICES.contains(v))
        {
            self.undo_send_seconds = v;
        }
        if let Some(v) = patch
            .send_later_hour
            .filter(|v| SEND_LATER_HOURS.contains(v))
        {
            self.send_later_hour = v;
        }
        if let Some(v) = patch.instant_replies {
            self.instant_replies = v;
        }
        if let Some(v) = patch.write_with_ai {
            self.write_with_ai = v;
        }
        if let Some(v) = patch.check_spelling {
            self.check_spelling = v;
        }
        if let Some(v) = patch.check_grammar {
            self.check_grammar = v;
        }
        if let Some(v) = patch
            .sync_window_months
            .filter(|v| SYNC_WINDOW_CHOICES.contains(v))
        {
            self.sync_window_months = v;
        }
        if let Some(v) = patch.older_mail {
            self.older_mail = v;
        }
        if let Some(v) = patch.show_shortcut_hints {
            self.show_shortcut_hints = v;
        }
        if let Some(v) = patch.shortcut_coach {
            self.shortcut_coach = v;
        }
        if let Some(v) = patch.unsubscribe_button {
            self.unsubscribe_button = v;
        }
        if let Some(v) = patch.semantic_search {
            self.semantic_search = v;
        }
        if let Some(v) = patch.ask_with_ai {
            self.ask_with_ai = v;
        }
        if let Some(v) = patch.summaries {
            self.summaries = v;
        }
        if let Some(v) = patch.sender_photos {
            self.sender_photos = v;
        }
        if let Some(v) = patch.avatar_placement {
            self.avatar_placement = v;
        }
        if let Some(v) = patch.calendar {
            self.calendar = v;
        }
        if let Some(v) = patch.notifications {
            self.notifications = v;
        }
        if let Some(v) = patch.smart_views {
            self.smart_views = v;
        }
        if let Some(v) = patch.welcome_completed {
            self.welcome_completed = v;
        }
        if let Some(v) = patch.me {
            if let Some(x) = v.name {
                self.me.name = x;
            }
            if let Some(x) = v.title {
                self.me.title = x;
            }
            if let Some(x) = v.company {
                self.me.company = x;
            }
            if let Some(x) = v.sign_off {
                self.me.sign_off = x;
            }
        }
        self.normalize();
    }

    /// Lowercase, trim, dedupe and bound the sender list; drop junk entries.
    fn normalize(&mut self) {
        let mut senders: Vec<String> = self
            .trusted_image_senders
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| s.contains('@') && s.len() <= 320 && !s.chars().any(char::is_whitespace))
            .collect();
        senders.sort();
        senders.dedup();
        senders.truncate(MAX_TRUSTED_SENDERS);
        self.trusted_image_senders = senders;
        self.profiles = normalize_profiles(std::mem::take(&mut self.profiles));
        self.hidden_from_all = normalize_account_ids(
            std::mem::take(&mut self.hidden_from_all),
            MAX_HIDDEN_FROM_ALL,
        );
        self.account_order =
            normalize_account_ids(std::mem::take(&mut self.account_order), MAX_ACCOUNT_ORDER);
        self.pending_setups = normalize_pending_setups(std::mem::take(&mut self.pending_setups));
        self.smart_views = normalize_smart_views(std::mem::take(&mut self.smart_views));
        self.inbox_splits = normalize_inbox_splits(std::mem::take(&mut self.inbox_splits));
        let accounts = std::mem::take(&mut self.notifications.accounts);
        self.notifications.accounts = accounts
            .into_iter()
            .map(|(k, v)| (k.trim().to_lowercase(), v))
            .filter(|(k, _)| k.contains('@') && k.len() <= 320)
            .take(MAX_NOTIFY_ACCOUNTS)
            .collect();
        let cal = &mut self.calendar;
        cal.past_months = cal
            .past_months
            .clamp(CALENDAR_PAST_MONTHS.0, CALENDAR_PAST_MONTHS.1);
        cal.future_months = cal
            .future_months
            .clamp(CALENDAR_FUTURE_MONTHS.0, CALENDAR_FUTURE_MONTHS.1);
        let me = &mut self.me;
        for f in [
            &mut me.name,
            &mut me.title,
            &mut me.company,
            &mut me.sign_off,
        ] {
            *f = me_field(f);
        }
        if me.photo.as_deref().is_some_and(|id| !is_photo_id(id)) {
            me.photo = None;
        }
        self.snippets = normalize_snippets(std::mem::take(&mut self.snippets));
        self.instant_replies = normalize_instant_replies(std::mem::take(&mut self.instant_replies));
        (self.signatures, self.signature_defaults) = normalize_signatures(
            std::mem::take(&mut self.signatures),
            std::mem::take(&mut self.signature_defaults),
        );
    }

    /// The settings as the UI sees them: profile members that aren't
    /// signed-in accounts are left out. The file keeps them until the
    /// profiles are next edited, so an account removed and added back in
    /// the meantime is still in its profiles.
    pub fn for_accounts(mut self, known: &HashSet<String>) -> Settings {
        for p in &mut self.profiles {
            p.account_ids.retain(|a| known.contains(a));
        }
        // Same for signature defaults and All Inboxes of removed accounts.
        self.signature_defaults.retain(|a, _| known.contains(a));
        self.hidden_from_all.retain(|a| known.contains(a));
        self.account_order.retain(|a| known.contains(a));
        self.notifications.accounts.retain(|a, _| known.contains(a));
        // A setup whose account now exists is finished; the UI's next write
        // drops it from the file too.
        self.pending_setups.retain(|p| !known.contains(&p.email));
        self
    }

    /// The tracking protections every message render applies.
    pub fn protection(&self) -> crate::views::Protection {
        crate::views::Protection {
            block_tracking_pixels: self.block_tracking_pixels,
            strip_link_tracking: self.strip_link_tracking,
        }
    }

    /// Whether `m` renders with remote images on first open.
    ///
    /// A trusted sender only counts when the message is authenticated as
    /// really coming from that address: From is attacker-controlled, and a
    /// spoofed "trusted" sender would otherwise learn the reader's IP and
    /// open time.
    ///
    /// Spam never loads them on its own, whatever the setting: a remote
    /// image there is how a spammer learns the address is read. Loading
    /// them by hand (load_remote_images) still works.
    pub fn remote_images_for(&self, m: &Message) -> RemoteImageDecision {
        if m.label_ids.iter().any(|l| l == "SPAM") {
            return RemoteImageDecision::Block;
        }
        match self.remote_images {
            RemoteImages::Always => RemoteImageDecision::Load,
            RemoteImages::Never => RemoteImageDecision::Block,
            RemoteImages::Ask => {
                let from = m.from.email.trim().to_lowercase();
                if self.trusted_image_senders.binary_search(&from).is_err() {
                    RemoteImageDecision::Block
                } else if sender_authenticated(m) {
                    RemoteImageDecision::Load
                } else {
                    RemoteImageDecision::BlockUnverifiedSender
                }
            }
        }
    }

    /// Missing file → defaults. An unreadable or malformed file is logged and
    /// replaced by defaults rather than keeping the app from starting; the
    /// next change rewrites it.
    pub fn load(path: &Path) -> Settings {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
            Err(e) => {
                tracing::warn!(error = %e, "could not read settings; using defaults");
                return Settings::default();
            }
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(mut s) => {
                s.normalize();
                s
            }
            Err(e) => {
                tracing::warn!(error = %e, "settings.json is malformed; using defaults");
                Settings::default()
            }
        }
    }

    /// Write to a temp file (0600), fsync, then rename over `path`.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{SETTINGS_FILE}.{}.tmp", std::process::id()));
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let result = (|| {
            let mut f = opts.open(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

/// Drop profiles without a valid id (and repeated ids), trim and bound names,
/// fall back to the default color for anything not `#rrggbb`, dedupe members
/// (lowercased, order kept) and keep at most MAX_PROFILES.
/// One entry per address (the last one wins), newest first, bounded; bad
/// addresses dropped, step and error text trimmed to size.
/// Trimmed, lowercased, first occurrence kept, blanks and overlong ids
/// dropped, at most `max` entries.
fn normalize_account_ids(ids: Vec<String>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in ids {
        let a = a.trim().to_lowercase();
        if !a.is_empty() && a.len() <= 320 && !out.contains(&a) {
            out.push(a);
        }
    }
    out.truncate(max);
    out
}

fn normalize_pending_setups(list: Vec<PendingSetup>) -> Vec<PendingSetup> {
    let mut out: Vec<PendingSetup> = Vec::new();
    for mut p in list.into_iter().rev() {
        p.email = p.email.trim().to_lowercase();
        if !p.email.contains('@') || p.email.len() > 320 || out.iter().any(|q| q.email == p.email) {
            continue;
        }
        p.step = p.step.trim().chars().take(MAX_PENDING_STEP_CHARS).collect();
        p.last_error = p
            .last_error
            .map(|e| {
                e.trim()
                    .chars()
                    .take(MAX_PENDING_ERROR_CHARS)
                    .collect::<String>()
            })
            .filter(|e| !e.is_empty());
        out.push(p);
    }
    out.sort_by_key(|p| std::cmp::Reverse(p.started_at));
    out.truncate(MAX_PENDING_SETUPS);
    out
}

fn normalize_profiles(profiles: Vec<Profile>) -> Vec<Profile> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for mut p in profiles {
        let id_ok = !p.id.is_empty()
            && p.id.len() <= 64
            && p.id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !id_ok || !seen.insert(p.id.clone()) {
            continue;
        }
        let name: String = p.name.split_whitespace().collect::<Vec<_>>().join(" ");
        p.name = if name.is_empty() {
            "Untitled".into()
        } else {
            name.chars().take(MAX_PROFILE_NAME).collect()
        };
        let hex = p.color.len() == 7
            && p.color.starts_with('#')
            && p.color[1..].chars().all(|c| c.is_ascii_hexdigit());
        p.color = if hex {
            p.color.to_ascii_lowercase()
        } else {
            DEFAULT_PROFILE_COLOR.into()
        };
        p.emoji = p
            .emoji
            .map(|e| e.trim().chars().take(MAX_EMOJI_CHARS).collect::<String>())
            .filter(|e| {
                !e.is_empty()
                    && !e
                        .chars()
                        .any(|c| c.is_control() || c.is_ascii_alphanumeric())
            });
        let mut members = Vec::new();
        for a in p.account_ids {
            let a = a.trim().to_lowercase();
            if !a.is_empty() && !members.contains(&a) {
                members.push(a);
            }
        }
        members.truncate(MAX_PROFILE_ACCOUNTS);
        p.account_ids = members;
        out.push(p);
        if out.len() == MAX_PROFILES {
            break;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteImageDecision {
    Load,
    Block,
    /// Trusted sender, but the message isn't authenticated as theirs.
    BlockUnverifiedSender,
}

/// Whether Google authenticated the From address (DMARC pass, or aligned
/// DKIM), per Gmail's receiving MX. Messages synced before this was recorded
/// read as unauthenticated until they're re-fetched.
fn sender_authenticated(m: &Message) -> bool {
    m.sender_authenticated
}

impl Settings {
    /// The sync engine's view of the window settings.
    pub fn window_policy(&self) -> penguin_provider::window::WindowPolicy {
        use penguin_provider::window::OlderMail as O;
        penguin_provider::window::WindowPolicy {
            months: self.sync_window_months,
            older: match self.older_mail {
                OlderMail::Headers => O::Headers,
                OlderMail::None => O::None,
                OlderMail::Full => O::Full,
            },
        }
    }
}

/// The in-memory copy plus where it lives. Updates are serialized by the
/// write lock, so two concurrent patches can't lose each other's fields.
pub struct SettingsState {
    path: PathBuf,
    current: RwLock<Settings>,
    /// The file on disk has no `welcomeCompleted` (missing, or written by a
    /// build from before the Welcome setup).
    predates_welcome: bool,
}

/// Whether settings.json at `path` was written before `welcomeCompleted`
/// existed (or doesn't exist). A malformed file counts as old.
fn predates_welcome(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("welcomeCompleted").cloned())
        .is_none()
}

impl SettingsState {
    pub fn load(config_dir: &Path) -> SettingsState {
        let path = config_dir.join(SETTINGS_FILE);
        let current = Settings::load(&path);
        SettingsState {
            predates_welcome: predates_welcome(&path),
            path,
            current: RwLock::new(current),
        }
    }

    /// At startup, before anything reads `welcome_completed`: an install
    /// from before the Welcome setup that already has accounts is treated as
    /// done (existing users don't get it, and their model isn't held back).
    /// A new install writes `welcomeCompleted: false` now, so an account
    /// added and a quit before the setup finishes still shows it next time.
    pub fn settle_welcome(&self, has_accounts: bool) -> std::io::Result<()> {
        if !self.predates_welcome {
            return Ok(());
        }
        let mut current = self.current.write().unwrap_or_else(|p| p.into_inner());
        let mut next = current.clone();
        next.welcome_completed = has_accounts;
        next.save(&self.path)?;
        *current = next;
        Ok(())
    }

    pub fn get(&self) -> Settings {
        self.current
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Apply and persist; the in-memory copy changes only if the write
    /// succeeded. Blocking (file I/O).
    pub fn update(&self, patch: SettingsPatch) -> std::io::Result<Settings> {
        let mut current = self.current.write().unwrap_or_else(|p| p.into_inner());
        let mut next = current.clone();
        next.apply(patch);
        next.save(&self.path)?;
        *current = next.clone();
        Ok(next)
    }

    /// Raise the agent level to Send (Settings → Developer, after the user
    /// confirmed the disclaimer). Persisted like `update`.
    pub fn grant_agent_send(&self) -> std::io::Result<Settings> {
        let mut current = self.current.write().unwrap_or_else(|p| p.into_inner());
        let mut next = current.clone();
        next.mcp.set_access(AgentAccess::Send);
        next.save(&self.path)?;
        *current = next.clone();
        Ok(next)
    }

    /// Point `me.photo` at a stored photo id (or none); persisted like
    /// `update`. Returns the settings and the id it replaced.
    pub fn set_me_photo(&self, id: Option<String>) -> std::io::Result<(Settings, Option<String>)> {
        let mut current = self.current.write().unwrap_or_else(|p| p.into_inner());
        let mut next = current.clone();
        let old = std::mem::replace(&mut next.me.photo, id);
        next.normalize();
        next.save(&self.path)?;
        *current = next.clone();
        Ok((next, old))
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn mcp_is_off_by_default_and_patchable() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(!s.mcp.enabled);
        let mut s = Settings::default();
        // The old switch still works: on = Read.
        s.apply(serde_json::from_str(r#"{"mcp":{"enabled":true}}"#).unwrap());
        assert!(s.mcp.enabled);
        assert_eq!(s.mcp.access, AgentAccess::Read);
        assert!(
            serde_json::from_str::<SettingsPatch>(r#"{"mcp":{"enabled":true,"sendTool":true}}"#)
                .is_ok(),
            "unknown keys inside mcp are ignored, not granted"
        );
        assert_eq!(
            serde_json::to_value(&s).unwrap()["mcp"],
            serde_json::json!({"access": "read", "enabled": true, "sendDelaySeconds": 60, "sendKnownOnly": true})
        );
    }

    #[test]
    fn agent_access_migrates_from_enabled_and_unknown_values_load_as_off() {
        let load = |json: &str| serde_json::from_str::<Settings>(json).unwrap().mcp;
        // Files from before levels.
        assert_eq!(load(r#"{"mcp":{"enabled":true}}"#).access, AgentAccess::Read);
        assert_eq!(load(r#"{"mcp":{"enabled":false}}"#).access, AgentAccess::Off);
        assert_eq!(load(r#"{"mcp":{}}"#).access, AgentAccess::Off);
        // Every level round-trips, with `enabled` derived.
        for a in [
            AgentAccess::Off,
            AgentAccess::Read,
            AgentAccess::Draft,
            AgentAccess::Send,
        ] {
            let text = serde_json::to_string(&McpSettings::with_access(a)).unwrap();
            let back: McpSettings = serde_json::from_str(&text).unwrap();
            assert_eq!(back.access, a);
            assert_eq!(back.enabled, a != AgentAccess::Off);
        }
        // `access` decides over `enabled`; an unknown value (a newer
        // build's, a typo, a wrong type) is Off, never a guess upward.
        for bad in [
            r#""sendEverything""#,
            r#""SEND""#,
            "3",
            "null",
            r#"{"level":"send"}"#,
        ] {
            let m = load(&format!(r#"{{"mcp":{{"access":{bad},"enabled":true}}}}"#));
            assert_eq!(m.access, AgentAccess::Off, "{bad}");
            assert!(!m.enabled);
        }
        assert_eq!(
            load(r#"{"mcp":{"access":"draft","enabled":false}}"#).access,
            AgentAccess::Draft
        );
        // A bad delay loads as the default; the rest of the section survives.
        let m = load(r#"{"mcp":{"access":"send","sendDelaySeconds":1,"sendKnownOnly":false}}"#);
        assert_eq!(
            (m.access, m.send_delay_seconds, m.send_known_only),
            (AgentAccess::Send, 60, false)
        );
        assert_eq!(
            load(r#"{"mcp":{"sendDelaySeconds":300}}"#).send_delay_seconds,
            300
        );
        // A non-object section doesn't sink the whole file.
        let s: Settings = serde_json::from_str(r#"{"mcp":true,"floeMode":true}"#).unwrap();
        assert_eq!(s.mcp.access, AgentAccess::Off);
        assert!(s.floe_mode);
    }

    #[test]
    fn a_patch_can_lower_the_agent_level_but_never_raise_it_to_send() {
        let patch = |json: &str| serde_json::from_str::<SettingsPatch>(json).unwrap();
        let mut s = Settings::default();
        s.apply(patch(r#"{"mcp":{"access":"draft"}}"#));
        assert_eq!(s.mcp.access, AgentAccess::Draft);
        assert!(patch(r#"{"mcp":{"access":"send"}}"#)
            .mcp
            .unwrap()
            .raises_to_send(s.mcp.access));
        s.apply(patch(r#"{"mcp":{"access":"send"}}"#));
        assert_eq!(
            s.mcp.access,
            AgentAccess::Draft,
            "send only through grant_agent_send"
        );
        // `enabled: true` doesn't lower an existing level.
        s.apply(patch(r#"{"mcp":{"enabled":true}}"#));
        assert_eq!(s.mcp.access, AgentAccess::Draft);
        // Once granted, a patch may restate it, and lowering works at once.
        let dir = temp_dir("agent-send");
        let state = SettingsState::load(&dir);
        state.update(patch(r#"{"mcp":{"access":"read"}}"#)).unwrap();
        let granted = state.grant_agent_send().unwrap();
        assert_eq!(granted.mcp.access, AgentAccess::Send);
        assert!(granted.mcp.enabled);
        assert_eq!(
            Settings::load(&dir.join(SETTINGS_FILE)).mcp.access,
            AgentAccess::Send
        );
        let same = state
            .update(patch(r#"{"mcp":{"access":"send","sendDelaySeconds":300}}"#))
            .unwrap();
        assert_eq!(
            (same.mcp.access, same.mcp.send_delay_seconds),
            (AgentAccess::Send, 300)
        );
        let lowered = state.update(patch(r#"{"mcp":{"access":"draft"}}"#)).unwrap();
        assert_eq!(lowered.mcp.access, AgentAccess::Draft);
        assert_eq!(state.get().mcp.access, AgentAccess::Draft);
        let off = state.update(patch(r#"{"mcp":{"enabled":false}}"#)).unwrap();
        assert_eq!((off.mcp.access, off.mcp.enabled), (AgentAccess::Off, false));
        // Delays outside the choices are ignored.
        let kept = state.update(patch(r#"{"mcp":{"sendDelaySeconds":0}}"#)).unwrap();
        assert_eq!(kept.mcp.send_delay_seconds, 300);
        let _ = std::fs::remove_dir_all(dir);
    }

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("penguin-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn from(email: &str) -> Message {
        Message {
            account_id: "ada@x.example".into(),
            id: "m1".into(),
            thread_id: "t1".into(),
            date: 0,
            from: penguin_core::Address {
                name: None,
                email: email.into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            reply_to: vec![],
            subject: String::new(),
            snippet: String::new(),
            body_text: String::new(),
            body_html: None,
            label_ids: vec![],
            attachments: vec![],
            message_id_header: None,
            in_reply_to: None,
            references: vec![],
            sender_authenticated: false,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
        }
    }

    #[test]
    fn follow_up_days_default_clamp_and_fallback() {
        assert_eq!(Settings::default().follow_up_days, 3);
        let s: Settings = serde_json::from_str(r#"{"followUpDays":7}"#).unwrap();
        assert_eq!(s.follow_up_days, 7);
        for bad in [
            r#"{"followUpDays":0}"#,
            r#"{"followUpDays":30}"#,
            r#"{"followUpDays":"x"}"#,
        ] {
            let s: Settings = serde_json::from_str(bad).unwrap();
            assert_eq!(s.follow_up_days, 3, "{bad}");
        }
        let mut s = Settings::default();
        s.apply(serde_json::from_str(r#"{"followUpDays":99}"#).unwrap());
        assert_eq!(s.follow_up_days, 14);
        s.apply(serde_json::from_str(r#"{"followUpDays":0}"#).unwrap());
        assert_eq!(s.follow_up_days, 1);
    }

    #[test]
    fn floe_and_compose_font_defaults_patch_and_fallback() {
        let s = Settings::default();
        assert_eq!(
            (s.floe_mode, s.compose_font, s.compose_font_size),
            (false, ComposeFont::Inter, 15)
        );
        let s: Settings = serde_json::from_str(
            r#"{"floeMode":true,"composeFont":"sourceSerif4","composeFontSize":18}"#,
        )
        .unwrap();
        assert_eq!(
            (s.floe_mode, s.compose_font, s.compose_font_size),
            (true, ComposeFont::SourceSerif4, 18)
        );
        // Unknown font / out-of-range size from a hand edit load as defaults.
        let s: Settings =
            serde_json::from_str(r#"{"composeFont":"comic","composeFontSize":99}"#).unwrap();
        assert_eq!(
            (s.compose_font, s.compose_font_size),
            (ComposeFont::Inter, 15)
        );
        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(r#"{"composeFont":"iaWriterQuattro","composeFontSize":40}"#)
                .unwrap(),
        );
        assert_eq!(
            (s.compose_font, s.compose_font_size),
            (ComposeFont::IaWriterQuattro, 20)
        );
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"composeFont":"comic"}"#).is_err());
        for (f, name) in [
            (ComposeFont::IbmPlexSans, "ibmPlexSans"),
            (ComposeFont::AtkinsonHyperlegible, "atkinsonHyperlegible"),
        ] {
            assert_eq!(serde_json::to_value(f).unwrap(), serde_json::json!(name));
        }
    }

    #[test]
    fn sidebar_text_size_clamps_and_falls_back() {
        let s: Settings = serde_json::from_str(r#"{"sidebarTextSize":-2}"#).unwrap();
        assert_eq!(s.sidebar_text_size, -2);
        // Out of range or not a number: the default, not an error.
        for raw in [r#"{"sidebarTextSize":9}"#, r#"{"sidebarTextSize":"big"}"#] {
            assert_eq!(serde_json::from_str::<Settings>(raw).unwrap().sidebar_text_size, 0);
        }
        let mut s = Settings::default();
        s.apply(serde_json::from_str(r#"{"sidebarTextSize":5}"#).unwrap());
        assert_eq!(s.sidebar_text_size, 2);
    }

    #[test]
    fn sidebar_theme_wire_names_patch_and_fallback() {
        let s: Settings =
            serde_json::from_str(r#"{"sidebarTheme":"lavender","matchAccent":true}"#).unwrap();
        assert_eq!(
            (s.sidebar_theme, s.match_accent),
            (SidebarTheme::Lavender, true)
        );
        // A theme from a newer build loads as Graphite instead of losing the file.
        let s: Settings =
            serde_json::from_str(r#"{"sidebarTheme":"neon","inboxTabs":true}"#).unwrap();
        assert_eq!(s.sidebar_theme, SidebarTheme::Graphite);
        assert!(s.inbox_tabs);
        let mut s = Settings::default();
        s.apply(serde_json::from_str(r#"{"sidebarTheme":"midnight","matchAccent":true}"#).unwrap());
        assert_eq!(
            (s.sidebar_theme, s.match_accent),
            (SidebarTheme::Midnight, true)
        );
        // …but update_settings rejects an unknown name outright.
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"sidebarTheme":"neon"}"#).is_err());
        for (t, name) in [
            (SidebarTheme::Arctic, "arctic"),
            (SidebarTheme::Sage, "sage"),
            (SidebarTheme::Butter, "butter"),
            (SidebarTheme::Mocha, "mocha"),
            (SidebarTheme::Crimson, "crimson"),
            (SidebarTheme::Indigo, "indigo"),
            (SidebarTheme::Plum, "plum"),
        ] {
            assert_eq!(serde_json::to_value(t).unwrap(), serde_json::json!(name));
        }
    }

    #[test]
    fn dark_shade_wire_names_patch_and_fallback() {
        assert_eq!(Settings::default().dark_shade, DarkShade::Black);
        let s: Settings = serde_json::from_str(r#"{"darkShade":"navy"}"#).unwrap();
        assert_eq!(s.dark_shade, DarkShade::Navy);
        // A shade from a newer build loads as Black instead of losing the file.
        let s: Settings = serde_json::from_str(r#"{"darkShade":"oled","inboxTabs":true}"#).unwrap();
        assert_eq!(s.dark_shade, DarkShade::Black);
        assert!(s.inbox_tabs);
        let mut s = Settings::default();
        s.apply(serde_json::from_str(r#"{"darkShade":"dim"}"#).unwrap());
        assert_eq!(s.dark_shade, DarkShade::Dim);
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"darkShade":"oled"}"#).is_err());
        for (v, name) in [
            (DarkShade::Black, "black"),
            (DarkShade::Charcoal, "charcoal"),
            (DarkShade::Dim, "dim"),
            (DarkShade::Navy, "navy"),
        ] {
            assert_eq!(serde_json::to_value(v).unwrap(), serde_json::json!(name));
        }
    }

    #[test]
    fn accent_color_and_corners_wire_names_patch_and_fallback() {
        let d = Settings::default();
        assert_eq!((d.accent_color, d.corners), (AccentColor::Blue, CornerStyle::Rounded));
        let s: Settings =
            serde_json::from_str(r#"{"accentColor":"teal","corners":"square"}"#).unwrap();
        assert_eq!((s.accent_color, s.corners), (AccentColor::Teal, CornerStyle::Square));
        // Names from a newer build load as the defaults instead of losing the file.
        let s: Settings =
            serde_json::from_str(r#"{"accentColor":"mauve","corners":"blob","inboxTabs":true}"#)
                .unwrap();
        assert_eq!((s.accent_color, s.corners), (AccentColor::Blue, CornerStyle::Rounded));
        assert!(s.inbox_tabs);
        let mut s = Settings::default();
        s.apply(serde_json::from_str(r#"{"accentColor":"graphite","corners":"subtle"}"#).unwrap());
        assert_eq!((s.accent_color, s.corners), (AccentColor::Graphite, CornerStyle::Subtle));
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"accentColor":"mauve"}"#).is_err());
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"corners":"blob"}"#).is_err());
        assert_eq!(serde_json::to_value(AccentColor::Yellow).unwrap(), serde_json::json!("yellow"));
        assert_eq!(serde_json::to_value(CornerStyle::Subtle).unwrap(), serde_json::json!("subtle"));
    }

    #[test]
    fn defaults_and_wire_format() {
        let s = Settings::default();
        assert_eq!(s.theme, ThemeSetting::System);
        assert_eq!(s.density, Density::Compact);
        assert!(!s.inbox_tabs);
        assert_eq!(s.remote_images, RemoteImages::Ask);
        let mut json = serde_json::to_value(&s).unwrap();
        // Checked on its own: one json! literal with every key hits the
        // macro recursion limit.
        let smart = json.as_object_mut().unwrap().remove("smartViews");
        let list_style = json.as_object_mut().unwrap().remove("listStyle");
        assert_eq!(list_style, Some(serde_json::json!("quiet")));
        assert_eq!(
            smart,
            Some(serde_json::json!({ "shown": [], "counts": [], "custom": [] }))
        );
        // Tracking protection: pixels blocked, links left as sent.
        let obj = json.as_object_mut().unwrap();
        assert_eq!(obj.remove("blockTrackingPixels"), Some(serde_json::json!(true)));
        assert_eq!(obj.remove("stripLinkTracking"), Some(serde_json::json!(false)));
        assert_eq!(obj.remove("shortcutCoach"), Some(serde_json::json!(true)));
        assert_eq!(obj.remove("requestReadReceipts"), Some(serde_json::json!(false)));
        // Spelling is checked while typing; grammar isn't (Safari's defaults).
        assert_eq!(obj.remove("checkSpelling"), Some(serde_json::json!(true)));
        assert_eq!(obj.remove("checkGrammar"), Some(serde_json::json!(false)));
        assert_eq!(obj.remove("sidebarTextSize"), Some(serde_json::json!(0)));
        // Ask reads questions with Apple Intelligence when the grammar can't.
        assert_eq!(obj.remove("askWithAi"), Some(serde_json::json!(true)));
        // A new install hasn't had the Welcome setup yet (docs/ONBOARDING.md).
        assert_eq!(obj.remove("welcomeCompleted"), Some(serde_json::json!(false)));
        // The Split Inbox's splits and Get to zero, likewise on their own.
        let splits = obj.remove("inboxSplits").unwrap();
        assert_eq!(
            splits[0],
            serde_json::json!({ "id": "important", "name": "Important", "query": "is:important -is:newsletter -has:invite", "hideWhenEmpty": false })
        );
        assert_eq!(splits.as_array().map(Vec::len), Some(3));
        assert_eq!(
            (obj.remove("getToZero"), obj.remove("zeroCelebration")),
            (Some(serde_json::json!(true)), Some(serde_json::json!(true)))
        );
        assert_eq!(
            json,
            serde_json::json!({
                "theme": "system", "density": "compact", "inboxTabs": false,
                "remoteImages": "ask", "trustedImageSenders": [],
                "profiles": [], "hiddenFromAll": [], "accountOrder": [], "swipeRight": "toggleRead", "swipeLeft": "archive", "swipeLeftLong": "trash",
                "sidebarTheme": "graphite", "matchAccent": false, "darkShade": "black", "darkEmailBodies": false,
                "accentColor": "blue", "corners": "rounded",
                "mcp": { "access": "off", "enabled": false, "sendDelaySeconds": 60, "sendKnownOnly": true },
                "floeMode": false, "composeFont": "inter", "composeFontSize": 15,
                "followUpDays": 3,
                "gmailUnitsPerMin": GMAIL_UNITS_PER_MIN_DEFAULT,
                "snippets": serde_json::to_value(default_snippets()).unwrap(), "undoSendSeconds": 10,
                "sendLaterHour": 8,
                "instantReplies": { "enabled": true, "replies": ["Sounds good, thanks!", "Thanks, got it.", "Let me check and get back to you."], "aiSuggestions": false },
                "writeWithAi": true,
                "signatures": [], "signatureDefaults": {},
                "signatureInsert": { "newMessages": true, "replies": true, "forwards": true },
                "signatureSeparator": false, "lockReplyAccount": true,
                "syncWindowMonths": 6, "olderMail": "headers", "showShortcutHints": false,
                "unsubscribeButton": true, "semanticSearch": true,
                "summaries": true,
                "senderPhotos": { "contacts": false, "bimi": true, "favicons": true, "gravatar": false },
                "avatarPlacement": "both",
                "me": { "name": "", "title": "", "company": "", "signOff": "", "photo": null },
                "calendar": { "pastMonths": 24, "futureMonths": 12, "nextUp": true, "connectOnSignIn": true },
                "notifications": { "enabled": false, "accounts": {}, "knownSendersOnly": false },
                "pendingSetups": []
            })
        );
        // Missing keys fall back to defaults; unknown keys are ignored.
        let partial: Settings = serde_json::from_str(r#"{"inboxTabs":true,"future":1}"#).unwrap();
        assert!(partial.inbox_tabs);
        // Settings files without the key get the default: hints off.
        assert!(!partial.show_shortcut_hints);
        let mut s = Settings::default();
        s.apply(serde_json::from_str(r#"{"showShortcutHints":true}"#).unwrap());
        assert!(s.show_shortcut_hints);
        // The coach is on for older files; the patch turns it off.
        assert!(partial.shortcut_coach);
        s.apply(serde_json::from_str(r#"{"shortcutCoach":false}"#).unwrap());
        assert!(!s.shortcut_coach);
        // Files from before the tracking settings keep today's behaviour.
        assert!(partial.block_tracking_pixels);
        assert!(!partial.strip_link_tracking);
        assert_eq!(partial.protection(), crate::views::Protection::default());
        s.apply(
            serde_json::from_str(r#"{"blockTrackingPixels":false,"stripLinkTracking":true}"#)
                .unwrap(),
        );
        assert_eq!(
            s.protection(),
            crate::views::Protection {
                block_tracking_pixels: false,
                strip_link_tracking: true
            }
        );
        // Settings files from before spell checking get it on, grammar off;
        // the patch turns each one on or off.
        assert!(partial.check_spelling);
        assert!(!partial.check_grammar);
        s.apply(serde_json::from_str(r#"{"checkSpelling":false,"checkGrammar":true}"#).unwrap());
        assert!(!s.check_spelling && s.check_grammar);
        s.apply(serde_json::from_str(r#"{"checkSpelling":true}"#).unwrap());
        assert!(s.check_spelling && s.check_grammar);
        // Read receipts are asked for only once turned on.
        assert!(!partial.request_read_receipts);
        s.apply(serde_json::from_str(r#"{"requestReadReceipts":true}"#).unwrap());
        assert!(s.request_read_receipts);
        // Settings files from before the unsubscribe button keep it on.
        assert!(partial.unsubscribe_button);
        s.apply(serde_json::from_str(r#"{"unsubscribeButton":false}"#).unwrap());
        assert!(!s.unsubscribe_button);
        assert!(partial.semantic_search);
        s.apply(serde_json::from_str(r#"{"semanticSearch":false}"#).unwrap());
        assert!(!s.semantic_search);
        // Summaries stay on for older settings files; the patch turns them off.
        assert!(partial.summaries);
        s.apply(serde_json::from_str(r#"{"summaries":false}"#).unwrap());
        assert!(!s.summaries);
        // Dark email bodies stay off for older settings files; the patch turns them on.
        assert!(!partial.dark_email_bodies);
        s.apply(serde_json::from_str(r#"{"darkEmailBodies":true}"#).unwrap());
        assert!(s.dark_email_bodies);
        assert_eq!(partial.remote_images, RemoteImages::Ask);
    }

    #[test]
    fn smart_views_are_off_by_default_and_round_trip() {
        // Older settings files (no section) and a malformed one: all off.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.smart_views, SmartViewSettings::default());
        let s: Settings = serde_json::from_str(r#"{"smartViews": 7}"#).unwrap();
        assert_eq!(s.smart_views, SmartViewSettings::default());

        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(
                r#"{"smartViews": {
                    "shown": ["bills", "receipts", "horoscopes", "bills", "custom:gone"],
                    "counts": ["bills", "nope", "custom:vip"],
                    "custom": [
                        {"id": "vip", "name": "  From   the board ", "query": " from:board@linden.example   is:unread "},
                        {"id": "vip", "name": "duplicate", "query": "x"},
                        {"id": "bad id!", "name": "x", "query": "x"},
                        {"id": "empty", "name": "x", "query": "   "},
                        {"id": "unnamed", "name": "", "query": "has:pdf"}
                    ]
                }}"#,
            )
            .unwrap(),
        );
        let v = &s.smart_views;
        // Order kept, unknown and repeated ids dropped, pinned searches shown.
        assert_eq!(
            v.shown,
            ["bills", "receipts", "custom:vip", "custom:unnamed"]
        );
        assert_eq!(v.counts, ["bills", "custom:vip"]);
        assert_eq!(
            v.custom,
            [
                CustomView {
                    id: "vip".into(),
                    name: "From the board".into(),
                    query: "from:board@linden.example is:unread".into()
                },
                CustomView {
                    id: "unnamed".into(),
                    name: "has:pdf".into(),
                    query: "has:pdf".into()
                },
            ]
        );
        // Through the file and back unchanged.
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.smart_views, s.smart_views);
        // Reordering is a new `shown`; turning one off leaves the rest.
        s.apply(
            serde_json::from_str(
                r#"{"smartViews": {"shown": ["custom:unnamed", "receipts", "custom:vip"], "counts": [], "custom": [
                    {"id": "vip", "name": "Board", "query": "from:board@linden.example"},
                    {"id": "unnamed", "name": "PDFs", "query": "has:pdf"}]}}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            s.smart_views.shown,
            ["custom:unnamed", "receipts", "custom:vip"]
        );
        assert!(s.smart_views.counts.is_empty());
        // A misspelled section name is rejected like any unknown patch key.
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"smartView": {}}"#).is_err());
    }

    #[test]
    fn split_inbox_starts_with_presets_and_normalizes() {
        // Older files have no list: the presets, with Split Inbox and get to zero defaults.
        let s: Settings = serde_json::from_str(r#"{"inboxTabs": true}"#).unwrap();
        assert!(s.inbox_tabs && s.get_to_zero && s.zero_celebration);
        let names: Vec<&str> = s.inbox_splits.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Important", "Calendar", "News"]);
        // Malformed: the presets again.
        let s: Settings = serde_json::from_str(r#"{"inboxSplits": "vip"}"#).unwrap();
        assert_eq!(s.inbox_splits, default_inbox_splits());

        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(
                r#"{"inboxSplits": [
                    {"id": "vip", "name": "  V I P  ", "query": " from:maya@acme.example  OR from:@board.example ", "hideWhenEmpty": true},
                    {"id": "vip", "name": "dup", "query": "x"},
                    {"id": "bad id", "name": "x", "query": "x"},
                    {"id": "blank", "name": "x", "query": "  "},
                    {"id": "news", "name": "", "query": "is:newsletter"}
                ], "getToZero": false, "zeroCelebration": false}"#,
            )
            .unwrap(),
        );
        assert!(!s.get_to_zero && !s.zero_celebration);
        assert_eq!(
            s.inbox_splits,
            [
                InboxSplit {
                    id: "vip".into(),
                    name: "V I P".into(),
                    query: "from:maya@acme.example OR from:@board.example".into(),
                    hide_when_empty: true
                },
                InboxSplit {
                    id: "news".into(),
                    name: "is:newsletter".into(),
                    query: "is:newsletter".into(),
                    hide_when_empty: false
                },
            ]
        );
        // An empty list stays empty (only Other), through the file and back.
        s.apply(serde_json::from_str(r#"{"inboxSplits": []}"#).unwrap());
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.inbox_splits.is_empty());
        // At most twelve.
        let many: Vec<InboxSplit> = (0..20)
            .map(|i| InboxSplit {
                id: format!("s{i}"),
                name: format!("S{i}"),
                query: "is:unread".into(),
                hide_when_empty: false,
            })
            .collect();
        s.apply(SettingsPatch {
            inbox_splits: Some(many),
            ..SettingsPatch::default()
        });
        assert_eq!(s.inbox_splits.len(), MAX_INBOX_SPLITS);
    }

    #[test]
    fn swipe_actions_default_per_direction_and_tolerate_unknown_names() {
        // Missing keys: each direction's own default.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            (s.swipe_right, s.swipe_left, s.swipe_left_long),
            (
                SwipeAction::ToggleRead,
                SwipeAction::Archive,
                SwipeAction::Trash
            )
        );
        // An unknown name (a newer build's action) falls back to that field's default.
        let s: Settings = serde_json::from_str(
            r#"{"swipeRight":"snooze","swipeLeft":"none","swipeLeftLong":"star"}"#,
        )
        .unwrap();
        assert_eq!(
            (s.swipe_right, s.swipe_left, s.swipe_left_long),
            (
                SwipeAction::ToggleRead,
                SwipeAction::None,
                SwipeAction::Star
            )
        );
        let mut s = Settings::default();
        s.apply(serde_json::from_str(r#"{"swipeLeft":"trash","swipeLeftLong":"none"}"#).unwrap());
        assert_eq!(
            (s.swipe_left, s.swipe_left_long),
            (SwipeAction::Trash, SwipeAction::None)
        );
    }

    #[test]
    fn missing_or_malformed_file_gives_defaults() {
        let dir = temp_dir("missing");
        assert_eq!(
            Settings::load(&dir.join(SETTINGS_FILE)),
            Settings::default()
        );
        std::fs::write(dir.join(SETTINGS_FILE), "{not json").unwrap();
        assert_eq!(
            Settings::load(&dir.join(SETTINGS_FILE)),
            Settings::default()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_persists_atomically_with_private_mode() {
        let dir = temp_dir("save");
        let state = SettingsState::load(&dir);
        let patch: SettingsPatch = serde_json::from_str(
            r#"{"remoteImages":"always","trustedImageSenders":[" Bo@X.example ","bo@x.example","junk"]}"#,
        )
        .unwrap();
        let saved = state.update(patch).unwrap();
        assert_eq!(saved.remote_images, RemoteImages::Always);
        assert_eq!(saved.trusted_image_senders, vec!["bo@x.example"]);
        // Untouched fields keep their values.
        assert_eq!(saved.density, Density::Compact);

        let reloaded = SettingsState::load(&dir).get();
        assert_eq!(reloaded, saved);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(SETTINGS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No temp files left behind.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(SETTINGS_FILE)]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_settings_file_from_the_app_icon_picker_still_loads() {
        // Settings → App icon was removed; files that saved a choice keep the
        // rest of their settings, and the UI can no longer patch it.
        let dir = temp_dir("retired-app-icon");
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"theme":"dark","appIcon":"tuxedo-split-b","inboxTabs":true}"#,
        )
        .unwrap();
        let s = Settings::load(&dir.join(SETTINGS_FILE));
        assert_eq!(s.theme, ThemeSetting::Dark);
        assert!(s.inbox_tabs);
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"appIcon":"ice-floe"}"#).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn gmail_quota_default_clamp_and_fallback() {
        assert_eq!(Settings::default().gmail_units_per_min, 6_000);
        let s: Settings = serde_json::from_str(r#"{"gmailUnitsPerMin":60000}"#).unwrap();
        assert_eq!(s.gmail_units_per_min, 60_000);
        // Out of range or malformed in the file: default, rest kept.
        let s: Settings =
            serde_json::from_str(r#"{"gmailUnitsPerMin":5,"inboxTabs":true}"#).unwrap();
        assert_eq!((s.gmail_units_per_min, s.inbox_tabs), (6_000, true));
        let s: Settings = serde_json::from_str(r#"{"gmailUnitsPerMin":"lots"}"#).unwrap();
        assert_eq!(s.gmail_units_per_min, 6_000);
        // A patch is clamped.
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            gmail_units_per_min: Some(10),
            ..Default::default()
        });
        assert_eq!(s.gmail_units_per_min, 600);
    }

    #[test]
    fn sender_photos_defaults_patch_and_partial_files() {
        let s = Settings::default();
        assert!(!s.sender_photos.gravatar && !s.sender_photos.contacts && s.sender_photos.bimi);
        assert!(s.avatar_prefs().show);
        // A file missing some keys keeps the rest at their defaults.
        let s: Settings =
            serde_json::from_str(r#"{"senderPhotos":{"gravatar":true,"favicons":false}}"#).unwrap();
        assert_eq!(
            s.sender_photos,
            SenderPhotos {
                gravatar: true,
                favicons: false,
                ..SenderPhotos::default()
            }
        );
        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(r#"{"senderPhotos":{"contacts":true,"bimi":true,"favicons":true,"gravatar":false}}"#)
                .unwrap(),
        );
        assert!(s.sender_photos.contacts && s.avatar_prefs().contacts);
    }

    #[test]
    fn calendar_defaults_patch_and_clamp() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.calendar, CalendarSettings::default());
        assert_eq!((s.calendar.past_months, s.calendar.future_months), (24, 12));
        let mut s = Settings::default();
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "calendar": {"pastMonths": 999, "futureMonths": 0, "nextUp": false, "connectOnSignIn": false}
        }))
        .unwrap();
        s.apply(patch);
        assert_eq!(
            s.calendar,
            CalendarSettings {
                past_months: 60,
                future_months: 1,
                next_up: false,
                connect_on_sign_in: false,
            }
        );
        // A partial section keeps the other defaults.
        let s: Settings = serde_json::from_str(r#"{"calendar": {"futureMonths": 6}}"#).unwrap();
        assert_eq!((s.calendar.past_months, s.calendar.future_months), (24, 6));
        // Settings files from before connectOnSignIn: calendar comes with sign-in.
        let s: Settings = serde_json::from_str(
            r#"{"calendar": {"pastMonths": 24, "futureMonths": 12, "nextUp": true}}"#,
        )
        .unwrap();
        assert!(s.calendar.connect_on_sign_in);
    }

    #[test]
    fn list_style_defaults_to_quiet_patches_and_falls_back() {
        for (v, name) in [
            (ListStyle::Quiet, "quiet"),
            (ListStyle::Classic, "classic"),
            (ListStyle::Contrast, "contrast"),
            (ListStyle::Cards, "cards"),
            (ListStyle::Mail, "mail"),
        ] {
            assert_eq!(serde_json::to_value(v).unwrap(), serde_json::json!(name));
        }
        // A settings.json from before the setting existed gets Quiet.
        let mut s: Settings = serde_json::from_str(r#"{"density":"comfortable"}"#).unwrap();
        assert_eq!(s.list_style, ListStyle::Quiet);
        s.apply(serde_json::from_str(r#"{"listStyle":"cards"}"#).unwrap());
        assert_eq!(s.list_style, ListStyle::Cards);
        // Round trip through the file format.
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.list_style, ListStyle::Cards);
        // An unknown name in the file loads as Quiet without losing the rest…
        let s: Settings =
            serde_json::from_str(r#"{"listStyle":"dense","inboxTabs":true}"#).unwrap();
        assert_eq!((s.list_style, s.inbox_tabs), (ListStyle::Quiet, true));
        // …but update_settings rejects it outright.
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"listStyle":"dense"}"#).is_err());
    }

    #[test]
    fn avatar_placement_wire_names_patch_and_fallback() {
        for (p, name) in [
            (AvatarPlacement::List, "list"),
            (AvatarPlacement::Message, "message"),
            (AvatarPlacement::Both, "both"),
            (AvatarPlacement::Off, "off"),
        ] {
            assert_eq!(serde_json::to_value(p).unwrap(), serde_json::json!(name));
        }
        let mut s = Settings::default();
        assert_eq!(s.avatar_placement, AvatarPlacement::Both);
        s.apply(serde_json::from_str(r#"{"avatarPlacement":"list"}"#).unwrap());
        assert_eq!(s.avatar_placement, AvatarPlacement::List);
        assert!(s.avatar_prefs().show);
        // Off stops the service entirely.
        s.apply(serde_json::from_str(r#"{"avatarPlacement":"off"}"#).unwrap());
        assert!(!s.avatar_prefs().show);
        // A placement from a newer build loads as Both; a patch naming one is refused.
        let s: Settings =
            serde_json::from_str(r#"{"avatarPlacement":"sidebar","inboxTabs":true}"#).unwrap();
        assert_eq!(
            (s.avatar_placement, s.inbox_tabs),
            (AvatarPlacement::Both, true)
        );
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"avatarPlacement":"sidebar"}"#).is_err());
    }

    #[test]
    fn me_defaults_patch_normalize_and_photo_is_not_patchable() {
        let s = Settings::default();
        assert_eq!(s.me, Me::default());
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json["me"],
            serde_json::json!({"name": "", "title": "", "company": "", "signOff": "", "photo": null})
        );
        let mut s = Settings::default();
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "me": {"name": "  Sam \n  Okafor ", "signOff": "Sam"}
        }))
        .unwrap();
        s.apply(patch);
        assert_eq!(
            (s.me.name.as_str(), s.me.sign_off.as_str()),
            ("Sam Okafor", "Sam")
        );
        let patch: SettingsPatch =
            serde_json::from_value(serde_json::json!({"me": {"title": "x".repeat(200)}})).unwrap();
        s.apply(patch);
        assert_eq!(s.me.title.chars().count(), MAX_ME_FIELD);
        assert_eq!(s.me.name, "Sam Okafor", "other fields kept");
        assert!(serde_json::from_value::<SettingsPatch>(
            serde_json::json!({"me": {"photo": "/etc/passwd"}})
        )
        .is_err());
        // A hand-edited photo id that isn't one of ours is dropped on load.
        let dir = temp_dir("me");
        let path = dir.join(SETTINGS_FILE);
        std::fs::write(&path, r#"{"me": {"name": "Sam", "photo": "../../x"}}"#).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!((loaded.me.name.as_str(), loaded.me.photo), ("Sam", None));
        let st = SettingsState::load(&dir);
        let id = "a".repeat(64);
        let (saved, old) = st.set_me_photo(Some(id.clone())).unwrap();
        assert_eq!((saved.me.photo.as_deref(), old), (Some(id.as_str()), None));
        assert_eq!(Settings::load(&path).me.photo, Some(id));
    }

    #[test]
    fn unknown_patch_fields_are_rejected() {
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"remoteImage":"always"}"#).is_err());
    }

    #[test]
    fn remote_image_policy() {
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            trusted_image_senders: Some(vec!["news@shop.example".into()]),
            ..Default::default()
        });
        // Trusted but unauthenticated: blocked, and flagged for the UI note.
        assert_eq!(
            s.remote_images_for(&from("News@Shop.example")),
            RemoteImageDecision::BlockUnverifiedSender
        );
        assert_eq!(
            s.remote_images_for(&from("other@shop.example")),
            RemoteImageDecision::Block
        );
        // Trusted and authenticated: loads.
        let mut authed = from("news@shop.example");
        authed.sender_authenticated = true;
        assert_eq!(s.remote_images_for(&authed), RemoteImageDecision::Load);
        // Authenticated but not trusted: still blocked.
        let mut stranger = from("other@shop.example");
        stranger.sender_authenticated = true;
        assert_eq!(s.remote_images_for(&stranger), RemoteImageDecision::Block);
        s.remote_images = RemoteImages::Never;
        assert_eq!(s.remote_images_for(&authed), RemoteImageDecision::Block);
        assert_eq!(
            s.remote_images_for(&from("news@shop.example")),
            RemoteImageDecision::Block
        );
        s.remote_images = RemoteImages::Always;
        assert_eq!(
            s.remote_images_for(&from("other@shop.example")),
            RemoteImageDecision::Load
        );
        // Spam: blocked under Always, and for a trusted, authenticated sender.
        let mut spam = from("other@shop.example");
        spam.label_ids = vec!["SPAM".into(), "UNREAD".into()];
        assert_eq!(s.remote_images_for(&spam), RemoteImageDecision::Block);
        s.remote_images = RemoteImages::Ask;
        let mut trusted_spam = authed.clone();
        trusted_spam.label_ids = vec!["SPAM".into()];
        assert_eq!(s.remote_images_for(&trusted_spam), RemoteImageDecision::Block);
    }

    fn profile(id: &str, name: &str, accounts: &[&str]) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
            color: "#2BD67B".into(),
            account_ids: accounts.iter().map(|a| a.to_string()).collect(),
            emoji: None,
        }
    }

    #[test]
    fn profiles_wire_format_and_defaults() {
        // Older files have no profiles key.
        let s: Settings = serde_json::from_str(r#"{"inboxTabs":true}"#).unwrap();
        assert!(s.profiles.is_empty());
        let s: Settings = serde_json::from_str(
            r##"{"profiles":[{"id":"hl","name":"Harbor Labs","color":"#aabbcc","accountIds":["team@harbor-labs.example"],"emoji":"🐧"}]}"##,
        )
        .unwrap();
        assert_eq!(s.profiles[0].account_ids, vec!["team@harbor-labs.example"]);
        assert_eq!(s.profiles[0].emoji.as_deref(), Some("🐧"));
        let json = serde_json::to_value(&s.profiles[0]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id":"hl","name":"Harbor Labs","color":"#aabbcc","accountIds":["team@harbor-labs.example"],"emoji":"🐧"})
        );
        // A patch can set profiles; unknown profile fields in a patch are tolerated.
        let patch: SettingsPatch =
            serde_json::from_str(r#"{"profiles":[{"id":"a","name":"A","future":1}]}"#).unwrap();
        assert_eq!(patch.profiles.unwrap().len(), 1);
    }

    #[test]
    fn pending_setups_are_lenient_normalized_and_dropped_once_added() {
        let old: Settings = serde_json::from_str(r#"{"inboxTabs":true}"#).unwrap();
        assert!(old.pending_setups.is_empty());
        let bad: Settings =
            serde_json::from_str(r#"{"pendingSetups":{"email":"a@x.example"},"inboxTabs":true}"#)
                .unwrap();
        assert!(bad.pending_setups.is_empty() && bad.inbox_tabs);
        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(
                r#"{"pendingSetups":[
                    {"email":"Sam@Yahoo.example","kind":"yahoo","step":"pw-create","startedAt":1},
                    {"email":"no-at-sign","kind":"yahoo","step":"pw-create","startedAt":2},
                    {"email":"ana@work.example","kind":"microsoft","step":"ms-signin","startedAt":4,"lastError":"  "},
                    {"email":"sam@yahoo.example","kind":"yahoo","step":"pw-connect","startedAt":5,"lastError":" wrong password "}
                ]}"#,
            )
            .unwrap(),
        );
        let emails: Vec<_> = s.pending_setups.iter().map(|p| p.email.as_str()).collect();
        assert_eq!(emails, ["sam@yahoo.example", "ana@work.example"]);
        assert_eq!(s.pending_setups[0].step, "pw-connect");
        assert_eq!(
            s.pending_setups[0].last_error.as_deref(),
            Some("wrong password")
        );
        assert_eq!(s.pending_setups[1].last_error, None);
        // Settings files keep a stored entry with an unknown kind out.
        let file: Settings = serde_json::from_str(
            r#"{"pendingSetups":[{"email":"lee@x.example","kind":"notAKind","step":"x","startedAt":3},
                {"email":"ana@work.example","kind":"microsoft","step":"ms-signin","startedAt":4}]}"#,
        )
        .unwrap();
        assert_eq!(file.pending_setups.len(), 1);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json["pendingSetups"][1],
            serde_json::json!({"email": "ana@work.example", "kind": "microsoft", "step": "ms-signin", "startedAt": 4, "lastError": null})
        );
        // Once the account is added, its setup is no longer pending.
        let view = s
            .clone()
            .for_accounts(&HashSet::from(["sam@yahoo.example".to_string()]));
        assert_eq!(view.pending_setups.len(), 1);
        assert_eq!(view.pending_setups[0].email, "ana@work.example");
        // Bounded.
        s.apply(SettingsPatch {
            pending_setups: Some(
                (0..50)
                    .map(|i| PendingSetup {
                        email: format!("u{i}@x.example"),
                        kind: crate::providers::SetupKind::Imap,
                        step: "imap-connect".into(),
                        started_at: i,
                        last_error: Some("e".repeat(1000)),
                    })
                    .collect(),
            ),
            ..Default::default()
        });
        assert_eq!(s.pending_setups.len(), MAX_PENDING_SETUPS);
        assert_eq!(s.pending_setups[0].email, "u49@x.example");
        assert_eq!(
            s.pending_setups[0]
                .last_error
                .as_ref()
                .unwrap()
                .chars()
                .count(),
            MAX_PENDING_ERROR_CHARS
        );
    }

    #[test]
    fn hidden_from_all_is_lenient_normalized_and_pruned() {
        // Older files have no key; a malformed value or entry is skipped.
        assert!(Settings::default().hidden_from_all.is_empty());
        let old: Settings = serde_json::from_str(r#"{"inboxTabs":true}"#).unwrap();
        assert!(old.hidden_from_all.is_empty());
        let bad: Settings =
            serde_json::from_str(r#"{"hiddenFromAll":"a@x.example","inboxTabs":true}"#).unwrap();
        assert!(bad.hidden_from_all.is_empty() && bad.inbox_tabs);
        let mut mixed: Settings =
            serde_json::from_str(r#"{"hiddenFromAll":[" B@X.example ",7,null,"b@x.example",""]}"#)
                .unwrap();
        mixed.normalize();
        assert_eq!(mixed.hidden_from_all, vec!["b@x.example"]);
        // The patch replaces the list, normalized the same way.
        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(
                r#"{"hiddenFromAll":["A@x.example","c@x.example","a@x.example"]}"#,
            )
            .unwrap(),
        );
        assert_eq!(s.hidden_from_all, vec!["a@x.example", "c@x.example"]);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json["hiddenFromAll"],
            serde_json::json!(["a@x.example", "c@x.example"])
        );
        // Removed accounts drop out of the UI's view (and so out of its next write).
        let view = s
            .clone()
            .for_accounts(&HashSet::from(["c@x.example".to_string()]));
        assert_eq!(view.hidden_from_all, vec!["c@x.example"]);
        s.apply(SettingsPatch {
            hidden_from_all: Some(view.hidden_from_all),
            ..Default::default()
        });
        assert_eq!(s.hidden_from_all, vec!["c@x.example"]);
        // Bounded.
        s.apply(SettingsPatch {
            hidden_from_all: Some((0..300).map(|i| format!("u{i}@x.example")).collect()),
            ..Default::default()
        });
        assert_eq!(s.hidden_from_all.len(), MAX_HIDDEN_FROM_ALL);
    }

    #[test]
    fn notifications_are_off_by_default_per_account_and_lenient() {
        let d = Settings::default();
        assert!(!d.notifications.enabled && !d.notifications.known_senders_only);
        assert!(!d.notifies_for("a@x.example"), "off until turned on");
        // Older files have no key; junk values and entries are skipped.
        let old: Settings = serde_json::from_str(r#"{"inboxTabs":true}"#).unwrap();
        assert_eq!(old.notifications, NotificationSettings::default());
        let bad: Settings = serde_json::from_str(
            r#"{"notifications":{"enabled":true,"accounts":{"a@x.example":"yes","b@x.example":false}}}"#,
        )
        .unwrap();
        assert!(bad.notifications.enabled);
        assert_eq!(
            bad.notifications.accounts,
            BTreeMap::from([("b@x.example".to_string(), false)])
        );
        let junk: Settings =
            serde_json::from_str(r#"{"notifications":{"enabled":true,"accounts":[1]}}"#).unwrap();
        assert!(junk.notifications.accounts.is_empty());
        // Every account by default, except the ones hidden from All Inboxes;
        // an explicit switch wins either way.
        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(
                r#"{"hiddenFromAll":["h@x.example","k@x.example"],"notifications":{"enabled":true,"accounts":{" K@X.example ":true,"a@x.example":false,"junk":true}}}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            s.notifications.accounts,
            BTreeMap::from([
                ("a@x.example".to_string(), false),
                ("k@x.example".to_string(), true)
            ])
        );
        assert!(s.notifies_for("b@x.example"));
        assert!(!s.notifies_for("a@x.example"));
        assert!(!s.notifies_for("h@x.example"));
        assert!(s.notifies_for("k@x.example"));
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json["notifications"],
            serde_json::json!({"enabled": true, "accounts": {"a@x.example": false, "k@x.example": true}, "knownSendersOnly": false})
        );
        // Removed accounts drop out of the UI's view.
        let view = s.for_accounts(&HashSet::from(["k@x.example".to_string()]));
        assert_eq!(view.notifications.accounts.len(), 1);
        // The master switch turns everything off.
        let mut off = view.clone();
        off.notifications.enabled = false;
        assert!(!off.notifies_for("k@x.example"));
    }

    #[test]
    fn account_order_is_lenient_normalized_and_pruned() {
        // Older files have no key; a malformed value or entry is skipped.
        assert!(Settings::default().account_order.is_empty());
        let old: Settings = serde_json::from_str(r#"{"inboxTabs":true}"#).unwrap();
        assert!(old.account_order.is_empty());
        let bad: Settings =
            serde_json::from_str(r#"{"accountOrder":{"a":1},"inboxTabs":true}"#).unwrap();
        assert!(bad.account_order.is_empty() && bad.inbox_tabs);
        let mut mixed: Settings = serde_json::from_str(
            r#"{"accountOrder":[" C@X.example ",7,null,"a@x.example","c@x.example",""]}"#,
        )
        .unwrap();
        mixed.normalize();
        // Order is kept (it is the point), duplicates keep their first place.
        assert_eq!(mixed.account_order, vec!["c@x.example", "a@x.example"]);
        // The patch replaces the list, normalized the same way.
        let mut s = Settings::default();
        s.apply(
            serde_json::from_str(
                r#"{"accountOrder":["B@x.example","a@x.example","gone@x.example","b@x.example"]}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            s.account_order,
            vec!["b@x.example", "a@x.example", "gone@x.example"]
        );
        assert_eq!(
            serde_json::to_value(&s).unwrap()["accountOrder"],
            serde_json::json!(["b@x.example", "a@x.example", "gone@x.example"])
        );
        // Signed-out accounts drop out of the UI's view, order kept.
        let view = s.clone().for_accounts(&HashSet::from([
            "a@x.example".to_string(),
            "b@x.example".to_string(),
        ]));
        assert_eq!(view.account_order, vec!["b@x.example", "a@x.example"]);
        // Bounded.
        s.apply(SettingsPatch {
            account_order: Some((0..300).map(|i| format!("u{i}@x.example")).collect()),
            ..Default::default()
        });
        assert_eq!(s.account_order.len(), MAX_ACCOUNT_ORDER);
        assert_eq!(s.account_order[0], "u0@x.example");
    }

    #[test]
    fn profiles_are_validated() {
        let mut s = Settings::default();
        let mut bad_color = profile(
            "hr",
            "  North   Wind ",
            &[" Hello@Northwind.example ", "hello@northwind.example", ""],
        );
        bad_color.color = "red".into();
        bad_color.emoji = Some("  ".into());
        let mut long = profile("long", &"x".repeat(100), &[]);
        long.emoji = Some("abc".into());
        s.apply(SettingsPatch {
            profiles: Some(vec![
                profile(
                    "hl",
                    "Harbor Labs",
                    &["a@harbor-labs.example", "b@harbor-labs.example"],
                ),
                profile("hl", "Duplicate id", &[]),
                profile("bad id!", "Bad id", &[]),
                profile("", "No id", &[]),
                bad_color,
                long,
                profile("blank", "   ", &[]),
            ]),
            ..Default::default()
        });
        let ids: Vec<&str> = s.profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["hl", "hr", "long", "blank"]);
        assert_eq!(s.profiles[0].name, "Harbor Labs");
        assert_eq!(s.profiles[0].color, "#2bd67b");
        assert_eq!(s.profiles[1].name, "North Wind");
        assert_eq!(s.profiles[1].color, DEFAULT_PROFILE_COLOR);
        assert_eq!(s.profiles[1].account_ids, vec!["hello@northwind.example"]);
        assert_eq!(s.profiles[1].emoji, None);
        assert_eq!(s.profiles[2].name.chars().count(), MAX_PROFILE_NAME);
        assert_eq!(s.profiles[2].emoji, None, "letters aren't an emoji");
        assert_eq!(s.profiles[3].name, "Untitled");

        let many: Vec<Profile> = (0..30)
            .map(|i| profile(&format!("p{i}"), "P", &[]))
            .collect();
        s.apply(SettingsPatch {
            profiles: Some(many),
            ..Default::default()
        });
        assert_eq!(s.profiles.len(), MAX_PROFILES);
    }

    #[test]
    fn unknown_accounts_are_pruned_for_the_ui_but_kept_on_disk() {
        let dir = temp_dir("profiles");
        let state = SettingsState::load(&dir);
        let saved = state
            .update(SettingsPatch {
                profiles: Some(vec![profile(
                    "hl",
                    "Harbor Labs",
                    &["a@harbor-labs.example", "gone@harbor-labs.example"],
                )]),
                ..Default::default()
            })
            .unwrap();
        let known: HashSet<String> = HashSet::from(["a@harbor-labs.example".to_string()]);
        let shown = saved.clone().for_accounts(&known);
        assert_eq!(shown.profiles[0].account_ids, vec!["a@harbor-labs.example"]);
        // The file still lists both until the next profiles edit.
        let reloaded = SettingsState::load(&dir).get();
        assert_eq!(
            reloaded.profiles[0].account_ids,
            vec!["a@harbor-labs.example", "gone@harbor-labs.example"]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn snippet(id: &str, trigger: &str) -> Snippet {
        Snippet {
            id: id.into(),
            trigger: trigger.into(),
            title: "T".into(),
            body: "Hi {first_name}".into(),
            ..Default::default()
        }
    }

    #[test]
    fn signatures_are_sanitized_and_defaults_pruned() {
        let mut s = Settings::default();
        let sig = |id: &str, html: &str| Signature {
            id: id.into(),
            name: format!("  {id}  "),
            html: html.into(),
        };
        s.apply(SettingsPatch {
            signatures: Some(vec![
                sig("work", r#"<p><strong>Ana Ruiz</strong><script>alert(1)</script> <a href="javascript:x">x</a> <a href="https://acme.example">acme</a><img src="https://t.example/p.gif"></p>"#),
                sig("work", "<p>dup</p>"),
                sig("bad id!", "<p>x</p>"),
                sig("home", &format!("<p>{}</p>", "a".repeat(MAX_SIGNATURE_HTML + 1))),
            ]),
            signature_defaults: Some(BTreeMap::from([
                ("Ana@Acme.example".into(), "work".into()),
                ("home@x.example".into(), "home".into()),
            ])),
            signature_separator: Some(true),
            ..Default::default()
        });
        assert_eq!(s.signatures.len(), 1);
        assert_eq!(s.signatures[0].name, "work");
        assert_eq!(
            s.signatures[0].html,
            r#"<p><strong>Ana Ruiz</strong> <a>x</a> <a href="https://acme.example">acme</a></p>"#
        );
        assert_eq!(
            s.signature_defaults,
            BTreeMap::from([("ana@acme.example".into(), "work".into())])
        );
        assert!(s.signature_separator);
        assert!(s.signature_insert.replies);
        // Loaded from an old file: defaults.
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(old.signatures.is_empty() && old.signature_insert.new_messages);
        // A hand-edited file: bad entries are skipped, the rest (and the other
        // settings) load; a partial insert object keeps its other defaults;
        // stored HTML is re-sanitized on load.
        let mut edited: Settings = serde_json::from_str(
            r#"{"inboxTabs":true,"signatures":[42,{"id":"ok","name":"Ok","html":"<p onclick=x>hi</p><script>x</script>"},{"id":7}],
                "signatureDefaults":{"a@x.example":"ok","b@x.example":3},"signatureInsert":{"replies":false}}"#,
        )
        .unwrap();
        edited.normalize();
        assert!(edited.inbox_tabs);
        assert_eq!(edited.signatures.len(), 1);
        assert_eq!(edited.signatures[0].html, "<p>hi</p>");
        assert_eq!(edited.signature_defaults.len(), 1);
        assert!(!edited.signature_insert.replies && edited.signature_insert.forwards);
        let bad_insert: Settings = serde_json::from_str(r#"{"signatureInsert":true}"#).unwrap();
        assert!(bad_insert.signature_insert.new_messages);
        // Removed accounts drop out of the defaults the UI sees.
        let view = edited.for_accounts(&HashSet::from(["b@x.example".to_string()]));
        assert!(view.signature_defaults.is_empty());
    }

    #[test]
    fn snippets_default_and_normalize() {
        let s = Settings::default();
        assert!(s.snippets.iter().any(|x| x.trigger == "thx"));
        // A file without the field gets the starters.
        let from_old: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(from_old.snippets, s.snippets);

        let mut s = Settings::default();
        s.apply(SettingsPatch {
            snippets: Some(vec![
                snippet("a", "Hi"),       // trigger lowercased
                snippet("b", "hi"),       // duplicate trigger: dropped
                snippet("c", "bad key!"), // invalid trigger: dropped
                snippet("bad id!", "ok"), // invalid id: dropped
                Snippet {
                    title: "x".repeat(200),
                    ..snippet("d", "long")
                },
            ]),
            ..Default::default()
        });
        let triggers: Vec<_> = s.snippets.iter().map(|x| x.trigger.as_str()).collect();
        assert_eq!(triggers, vec!["hi", "long"]);
        assert_eq!(s.snippets[1].title.chars().count(), MAX_SNIPPET_TITLE);
    }

    #[test]
    fn undo_send_seconds_only_takes_the_offered_choices() {
        let mut s = Settings::default();
        assert_eq!(s.undo_send_seconds, 10);
        s.apply(SettingsPatch {
            undo_send_seconds: Some(30),
            ..Default::default()
        });
        assert_eq!(s.undo_send_seconds, 30);
        s.apply(SettingsPatch {
            undo_send_seconds: Some(7),
            ..Default::default()
        });
        assert_eq!(s.undo_send_seconds, 30);
        let bad: Settings = serde_json::from_str(r#"{"undoSendSeconds":99}"#).unwrap();
        assert_eq!(bad.undo_send_seconds, 10);
        // 0 turns undo send off.
        s.apply(SettingsPatch {
            undo_send_seconds: Some(0),
            ..Default::default()
        });
        assert_eq!(s.undo_send_seconds, 0);
    }

    #[test]
    fn send_later_hour_is_a_morning_hour() {
        let mut s = Settings::default();
        assert_eq!(s.send_later_hour, 8);
        s.apply(SettingsPatch {
            send_later_hour: Some(9),
            ..Default::default()
        });
        assert_eq!(s.send_later_hour, 9);
        s.apply(SettingsPatch {
            send_later_hour: Some(15),
            ..Default::default()
        });
        assert_eq!(s.send_later_hour, 9);
        let bad: Settings = serde_json::from_str(r#"{"sendLaterHour":"eight"}"#).unwrap();
        assert_eq!(bad.send_later_hour, 8);
    }

    #[test]
    fn snippets_keep_cc_bcc_subject_and_files_within_limits() {
        let hex = |c: char| c.to_string().repeat(64);
        let file = |id: String| SnippetFile {
            id,
            filename: "  Pricing.pdf ".into(),
            mime_type: "application/pdf".into(),
            size: 1200,
        };
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            snippets: Some(vec![Snippet {
                subject: format!("  {}", "s".repeat(300)),
                cc: vec![
                    " Dana <dana@northwind.example> ".into(),
                    "dana@northwind.example".into(),
                    "".into(),
                    "DANA <dana@northwind.example>".into(),
                ],
                bcc: (0..30).map(|i| format!("p{i}@harbor.example")).collect(),
                attachments: vec![
                    file(hex('a')),
                    file(hex('a')),
                    file("not-a-hash".into()),
                    file(hex('B')),
                    file(hex('b')),
                ],
                ..snippet("intro", "intro")
            }]),
            ..Default::default()
        });
        let sn = &s.snippets[0];
        assert_eq!(sn.subject.chars().count(), MAX_SNIPPET_SUBJECT);
        assert_eq!(
            sn.cc,
            vec!["Dana <dana@northwind.example>", "dana@northwind.example"]
        );
        assert_eq!(sn.bcc.len(), MAX_SNIPPET_ADDRESSES);
        let ids: Vec<_> = sn.attachments.iter().map(|f| f.id.clone()).collect();
        assert_eq!(ids, vec![hex('a'), hex('b')]);
        assert_eq!(sn.attachments[0].filename, "Pricing.pdf");
        // Snippets saved before these fields existed load with none.
        let old: Settings = serde_json::from_str(
            r#"{"snippets":[{"id":"x","trigger":"x","title":"X","body":"Hi","uses":2}]}"#,
        )
        .unwrap();
        assert!(
            old.snippets[0].cc.is_empty()
                && old.snippets[0].attachments.is_empty()
                && old.snippets[0].subject.is_empty()
        );
    }

    #[test]
    fn instant_replies_default_and_normalize() {
        let s = Settings::default();
        assert!(s.instant_replies.enabled && !s.instant_replies.ai_suggestions);
        assert_eq!(s.instant_replies.replies.len(), 3);
        assert!(s.write_with_ai);
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            instant_replies: Some(InstantReplies {
                enabled: true,
                replies: std::iter::once("  Sounds good ".to_string())
                    .chain(["Sounds good".into(), "".into(), "x".repeat(300)])
                    .chain((0..20).map(|i| format!("Reply {i}")))
                    .collect(),
                ai_suggestions: true,
            }),
            write_with_ai: Some(false),
            ..Default::default()
        });
        let r = &s.instant_replies;
        assert_eq!(r.replies.len(), MAX_INSTANT_REPLIES);
        assert_eq!(r.replies[0], "Sounds good");
        assert_eq!(r.replies[1].chars().count(), MAX_INSTANT_REPLY);
        assert!(r.ai_suggestions);
        assert!(!s.write_with_ai);
        // A malformed section loads as the defaults; a partial one keeps the rest.
        let bad: Settings = serde_json::from_str(r#"{"instantReplies":"yes"}"#).unwrap();
        assert_eq!(bad.instant_replies, InstantReplies::default());
        let partial: Settings =
            serde_json::from_str(r#"{"instantReplies":{"aiSuggestions":true}}"#).unwrap();
        assert!(partial.instant_replies.enabled && partial.instant_replies.ai_suggestions);
    }

    #[test]
    fn sync_window_settings_default_validate_and_map_to_the_engine() {
        use penguin_provider::window::{OlderMail as O, WindowPolicy};
        let mut s = Settings::default();
        assert_eq!(
            (s.sync_window_months, s.older_mail),
            (6, OlderMail::Headers)
        );
        s.apply(SettingsPatch {
            sync_window_months: Some(0),
            older_mail: Some(OlderMail::None),
            ..Default::default()
        });
        assert_eq!(
            s.window_policy(),
            WindowPolicy {
                months: 0,
                older: O::None
            }
        );
        s.apply(SettingsPatch {
            sync_window_months: Some(5),
            ..Default::default()
        });
        assert_eq!(s.sync_window_months, 0, "only offered choices");
        let bad: Settings =
            serde_json::from_str(r#"{"syncWindowMonths":7,"olderMail":"sometimes"}"#).unwrap();
        assert_eq!(
            (bad.sync_window_months, bad.older_mail),
            (6, OlderMail::Headers)
        );
        let full: Settings =
            serde_json::from_str(r#"{"olderMail":"full","syncWindowMonths":24}"#).unwrap();
        assert_eq!(
            full.window_policy(),
            WindowPolicy {
                months: 24,
                older: O::Full
            }
        );
    }

    #[test]
    fn welcome_shows_on_new_installs_only() {
        // A new install: no file, no accounts. Not done, and written down
        // so a quit before the setup finishes still shows it next time.
        let dir = temp_dir("welcome-new");
        let state = SettingsState::load(&dir);
        assert!(!state.get().welcome_completed);
        state.settle_welcome(false).unwrap();
        assert!(!state.get().welcome_completed);
        let reloaded = SettingsState::load(&dir);
        reloaded.settle_welcome(true).unwrap();
        assert!(!reloaded.get().welcome_completed, "an account added mid-setup keeps it pending");
        // Finished (or skipped): stays done.
        let patch: SettingsPatch = serde_json::from_str(r#"{"welcomeCompleted":true}"#).unwrap();
        assert!(reloaded.update(patch).unwrap().welcome_completed);
        assert!(SettingsState::load(&dir).get().welcome_completed);
        std::fs::remove_dir_all(dir).unwrap();

        // An existing install: a file from before the setting, with accounts.
        let dir = temp_dir("welcome-old");
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"theme":"dark","semanticSearch":false}"#).unwrap();
        let state = SettingsState::load(&dir);
        state.settle_welcome(true).unwrap();
        let s = SettingsState::load(&dir).get();
        assert!(s.welcome_completed);
        assert_eq!(s.theme, ThemeSetting::Dark);
        assert!(!s.semantic_search, "other settings are kept");
        std::fs::remove_dir_all(dir).unwrap();

        // An old file but no accounts yet: the setup still comes.
        let dir = temp_dir("welcome-old-empty");
        std::fs::write(dir.join(SETTINGS_FILE), "{}").unwrap();
        let state = SettingsState::load(&dir);
        state.settle_welcome(false).unwrap();
        assert!(!SettingsState::load(&dir).get().welcome_completed);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
