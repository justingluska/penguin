//! The sync-window policy every provider follows (Settings → Sync): the last
//! `months` of mail is downloaded in full, older mail per [`OlderMail`].
//! Moved from penguin-gmail's `sync_window.rs` (which re-exports it); the
//! Gmail-specific fill logic stays there. Progress is recorded in the
//! generic `SyncCursor.window` (`WindowCursor`) so coverage, diagnostics and
//! "free up space" work the same for every provider.

pub const DAY_MS: i64 = 86_400_000;
/// Average Gregorian month, for "last N months".
pub const MONTH_MS: f64 = 30.436_875 * DAY_MS as f64;
/// Allowed `syncWindowMonths` values; 0 = everything.
pub const WINDOW_MONTH_CHOICES: [u32; 6] = [1, 3, 6, 12, 24, 0];
pub const WINDOW_MONTHS_DEFAULT: u32 = 6;

/// What happens to mail older than the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OlderMail {
    /// Headers, labels and snippet only, after the window is complete.
    Headers,
    /// Not downloaded at all (still reachable via server search).
    None,
    /// Downloaded in full, after the window is complete.
    Full,
}

impl OlderMail {
    /// The name stored in `WindowCursor::older_mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            OlderMail::Headers => "headers",
            OlderMail::None => "none",
            OlderMail::Full => "full",
        }
    }
}

/// The sync-window settings the engines follow (live; see
/// `Backend::set_window_policy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPolicy {
    /// Months of mail downloaded in full; 0 = everything.
    pub months: u32,
    pub older: OlderMail,
}

impl Default for WindowPolicy {
    fn default() -> Self {
        WindowPolicy {
            months: WINDOW_MONTHS_DEFAULT,
            older: OlderMail::Headers,
        }
    }
}

impl WindowPolicy {
    /// Everything in full, the pre-window behavior (and the default for a
    /// bare engine, so engine-less tests keep their semantics).
    pub const EVERYTHING: WindowPolicy = WindowPolicy {
        months: 0,
        older: OlderMail::Full,
    };
}

/// Unix ms where a `months`-month window starts (0 = everything), rounded
/// down to UTC midnight so listing queries stay stable within a day.
pub fn window_start_ms(now_ms: i64, months: u32) -> i64 {
    if months == 0 {
        return 0;
    }
    let start = now_ms - (months as f64 * MONTH_MS) as i64;
    (start.div_euclid(DAY_MS) * DAY_MS).max(0)
}
