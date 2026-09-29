//! When a sync failure is worth telling the user about.
//!
//! Every engine retries on its own after an error (backoff 10 s, 20 s,
//! 40 s … 5 min), and most failures are gone by the next attempt: a Wi-Fi
//! hand-off, a Mac waking before the network is up, a server dropping a
//! connection. Before this, the first failed attempt turned the account's
//! status into `Error` and the UI put up "Can't reach Yahoo right now" at
//! once, only for the next attempt to take it away again ten seconds later.
//!
//! Now each engine records its attempts on the status:
//! - `record_failure` on every failed attempt. It opens or extends the
//!   account's [`SyncFailure`] streak and marks it `alert` when it needs
//!   the user (signed out, Keychain, a crash, bad settings: nothing will
//!   retry by itself) or has lasted: [`SYNC_ALERT_FAILURES`] attempts in a
//!   row, or [`SYNC_ALERT_AFTER_MS`] since the first. Until then the UI
//!   says "Retrying…" quietly.
//! - `record_progress` when sync stores or checks mail again. It closes the
//!   streak into `recovered`, so the UI can say sync is back instead of an
//!   alert silently vanishing.
//! - `retry_now` when the user asks for a retry: the streak stays open
//!   until that attempt succeeds or fails, so the UI can say which.
//!
//! Connecting alone is not progress: an engine that logs in fine and then
//! fails the same step every time must still reach the alert.

use crate::types::{SyncErrorKind, SyncFailure, SyncPhase, SyncRecovery, SyncStatus};

/// Failed attempts in a row before the UI alerts (about 30 s at the
/// engines' 10 s / 20 s backoff).
pub const SYNC_ALERT_FAILURES: u32 = 3;
/// …or this long since the first failure, whichever comes first (slow
/// failures, like a connect that times out after 30 s each time).
pub const SYNC_ALERT_AFTER_MS: i64 = 60_000;

impl SyncErrorKind {
    /// Sync can't resume until the user signs in again.
    pub fn needs_sign_in(self) -> bool {
        matches!(self, SyncErrorKind::Auth | SyncErrorKind::Keychain)
    }
}

impl SyncStatus {
    /// A status with nothing known yet beyond its phase.
    pub fn new(account_id: impl Into<String>, phase: SyncPhase) -> SyncStatus {
        SyncStatus {
            account_id: account_id.into(),
            phase,
            indexed: 0,
            total_estimate: None,
            last_synced_at: None,
            error: None,
            rate_per_min: None,
            eta_secs: None,
            stage: None,
            failure: None,
            recovered: None,
        }
    }

    /// A sync attempt failed. `retry_in_ms`: when the engine tries again on
    /// its own (None: it stops until someone acts). The phase becomes
    /// NeedsReauth for sign-in problems, Error otherwise.
    pub fn record_failure(
        &mut self,
        kind: SyncErrorKind,
        message: String,
        now_ms: i64,
        retry_in_ms: Option<i64>,
    ) {
        let (count, first_at) = match &self.failure {
            Some(f) => (f.count.saturating_add(1), f.first_at),
            None => (1, now_ms),
        };
        let next_retry_at = retry_in_ms.map(|ms| now_ms + ms.max(0));
        let alert = kind.needs_sign_in()
            || next_retry_at.is_none()
            || count >= SYNC_ALERT_FAILURES
            || now_ms - first_at >= SYNC_ALERT_AFTER_MS;
        self.failure = Some(SyncFailure {
            kind,
            count,
            first_at,
            last_at: now_ms,
            next_retry_at,
            // Once alerted, a streak stays alerted until it ends.
            alert: alert || self.failure.as_ref().is_some_and(|f| f.alert),
        });
        self.recovered = None;
        self.phase = if kind.needs_sign_in() {
            SyncPhase::NeedsReauth
        } else {
            SyncPhase::Error
        };
        self.error = Some(message);
        self.rate_per_min = None;
        self.eta_secs = None;
    }

    /// Sync stored or checked mail: any failure streak is over. The caller
    /// sets the phase and error (a health note) as usual.
    pub fn record_progress(&mut self, now_ms: i64) {
        if let Some(f) = self.failure.take() {
            self.recovered = Some(SyncRecovery {
                at: now_ms,
                since: f.first_at,
                failures: f.count,
                kind: f.kind,
                alerted: f.alert,
            });
        }
    }

    /// The user asked for a retry and the engine was woken: it tries now.
    pub fn retry_now(&mut self, now_ms: i64) {
        if let Some(f) = self.failure.as_mut() {
            f.next_retry_at = Some(now_ms);
        }
    }

    /// A failure streak is open and nothing has synced since.
    pub fn failing(&self) -> bool {
        self.failure.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_790_000_000_000;

    fn status() -> SyncStatus {
        let mut s = SyncStatus::new("sam@lark.example", SyncPhase::Idle);
        s.last_synced_at = Some(T0 - 5_000);
        s
    }

    #[test]
    fn a_single_failure_is_quiet_and_recovers() {
        let mut s = status();
        s.record_failure(
            SyncErrorKind::Network,
            "network: connection closed".into(),
            T0,
            Some(10_000),
        );
        let f = s.failure.clone().unwrap();
        assert_eq!(s.phase, SyncPhase::Error);
        assert_eq!(
            (f.count, f.first_at, f.next_retry_at, f.alert),
            (1, T0, Some(T0 + 10_000), false)
        );
        s.record_progress(T0 + 10_500);
        assert!(s.failure.is_none());
        let r = s.recovered.clone().unwrap();
        assert_eq!(
            (r.at, r.since, r.failures, r.alerted),
            (T0 + 10_500, T0, 1, false)
        );
    }

    #[test]
    fn the_third_failure_in_a_row_alerts() {
        let mut s = status();
        for (i, at) in [T0, T0 + 10_000, T0 + 30_000].into_iter().enumerate() {
            s.record_failure(
                SyncErrorKind::Network,
                "network: timed out".into(),
                at,
                Some(10_000),
            );
            let f = s.failure.clone().unwrap();
            assert_eq!(f.count, i as u32 + 1);
            assert_eq!(f.first_at, T0);
            assert_eq!(f.alert, i == 2, "attempt {}", i + 1);
        }
    }

    #[test]
    fn a_slow_second_failure_alerts_after_a_minute() {
        let mut s = status();
        s.record_failure(
            SyncErrorKind::Network,
            "network: timed out".into(),
            T0,
            Some(10_000),
        );
        s.record_failure(
            SyncErrorKind::Network,
            "network: timed out".into(),
            T0 + 59_999,
            Some(20_000),
        );
        assert!(!s.failure.as_ref().unwrap().alert);
        let mut s = status();
        s.record_failure(
            SyncErrorKind::Server,
            "http 503: busy".into(),
            T0,
            Some(10_000),
        );
        s.record_failure(
            SyncErrorKind::Server,
            "http 503: busy".into(),
            T0 + 61_000,
            Some(20_000),
        );
        assert!(s.failure.as_ref().unwrap().alert);
    }

    #[test]
    fn sign_in_problems_and_dead_tasks_alert_at_once() {
        let mut s = status();
        s.record_failure(
            SyncErrorKind::Auth,
            "account needs to sign in again: bad password".into(),
            T0,
            None,
        );
        assert_eq!(s.phase, SyncPhase::NeedsReauth);
        assert!(s.failure.as_ref().unwrap().alert);

        let mut s = status();
        s.record_failure(
            SyncErrorKind::Keychain,
            "Keychain access failed: denied".into(),
            T0,
            None,
        );
        assert_eq!(s.phase, SyncPhase::NeedsReauth);
        assert!(s.failure.as_ref().unwrap().alert);

        // A crashed task never retries by itself.
        let mut s = status();
        s.record_failure(
            SyncErrorKind::Internal,
            "internal sync error (boom)".into(),
            T0,
            None,
        );
        assert_eq!(s.phase, SyncPhase::Error);
        assert!(s.failure.as_ref().unwrap().alert);
    }

    #[test]
    fn an_auth_error_during_a_quiet_streak_alerts() {
        let mut s = status();
        s.record_failure(
            SyncErrorKind::Network,
            "network: timed out".into(),
            T0,
            Some(10_000),
        );
        s.record_failure(
            SyncErrorKind::Auth,
            "account needs to sign in again".into(),
            T0 + 10_000,
            None,
        );
        let f = s.failure.clone().unwrap();
        assert_eq!((f.kind, f.count, f.alert), (SyncErrorKind::Auth, 2, true));
    }

    #[test]
    fn an_alerted_streak_stays_alerted_and_recovery_says_so() {
        let mut s = status();
        for at in [T0, T0 + 10_000, T0 + 30_000, T0 + 70_000] {
            s.record_failure(
                SyncErrorKind::Network,
                "network: timed out".into(),
                at,
                Some(10_000),
            );
        }
        assert!(s.failure.as_ref().unwrap().alert);
        s.record_progress(T0 + 80_000);
        let r = s.recovered.clone().unwrap();
        assert_eq!(
            (r.failures, r.alerted, r.kind),
            (4, true, SyncErrorKind::Network)
        );
        // The next failure starts a new, quiet streak and forgets the recovery.
        s.record_failure(
            SyncErrorKind::Network,
            "network: timed out".into(),
            T0 + 200_000,
            Some(10_000),
        );
        let f = s.failure.clone().unwrap();
        assert_eq!((f.count, f.first_at, f.alert), (1, T0 + 200_000, false));
        assert!(s.recovered.is_none());
    }

    #[test]
    fn retry_now_keeps_the_streak_open_until_the_attempt_ends() {
        let mut s = status();
        s.record_failure(
            SyncErrorKind::Network,
            "network: timed out".into(),
            T0,
            Some(40_000),
        );
        s.retry_now(T0 + 2_000);
        let f = s.failure.clone().unwrap();
        assert_eq!((f.count, f.next_retry_at), (1, Some(T0 + 2_000)));
        assert_eq!(s.phase, SyncPhase::Error);
        // Healthy statuses are untouched.
        let mut ok = status();
        ok.retry_now(T0);
        assert!(ok.failure.is_none());
    }

    #[test]
    fn progress_without_a_streak_changes_nothing() {
        let mut s = status();
        s.record_progress(T0);
        assert!(s.failure.is_none() && s.recovered.is_none());
    }

    #[test]
    fn statuses_without_the_new_fields_still_parse() {
        let old = r#"{"accountId":"a","phase":"error","indexed":1,"totalEstimate":null,"lastSyncedAt":null,"error":"x"}"#;
        let s: SyncStatus = serde_json::from_str(old).unwrap();
        assert!(s.failure.is_none() && s.recovered.is_none());
    }
}
