//! The sync engines' progress heartbeat: while an account backfills, its
//! status goes out at least once a second so the UI's rate and ETA keep
//! moving between stored chunks. Outside a backfill it holds no timer.
//!
//! The first version ticked once a second for the life of every account's
//! task and checked the phase on each tick: 3,600 wakeups an hour per
//! account while the app sat idle, each for nothing. Apple: "Timers prevent
//! the CPU from going to or staying in the idle state" (Energy Efficiency
//! Guide for Mac Apps, "Minimize Timer Usage"). Here the task waits on the
//! phase itself (a `watch` channel) and ticks only while it is Backfilling.

use std::time::Duration;

use penguin_core::SyncPhase;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

/// Whether an account is backfilling, as the heartbeat sees it. The engine
/// reports every phase change through [`Heartbeat::phase`].
#[derive(Debug)]
pub struct Heartbeat {
    backfilling: watch::Sender<bool>,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Heartbeat {
            backfilling: watch::Sender::new(false),
        }
    }
}

impl Heartbeat {
    /// The account's phase is now `phase` (call on every status update;
    /// only a change into or out of Backfilling wakes the heartbeat).
    pub fn phase(&self, phase: SyncPhase) {
        let on = phase == SyncPhase::Backfilling;
        self.backfilling.send_if_modified(|b| {
            let changed = *b != on;
            *b = on;
            changed
        });
    }

    /// Call `beat` every `every` while backfilling, starting as a backfill
    /// begins; idle otherwise. Never returns (select it against the sync
    /// loop, as the engines do).
    pub async fn run(&self, every: Duration, mut beat: impl FnMut()) {
        let mut rx = self.backfilling.subscribe();
        loop {
            // Asleep, with no timer, until a backfill starts. The sender
            // lives in `self`, so this can't see a closed channel.
            if rx.wait_for(|b| *b).await.is_err() {
                return std::future::pending().await;
            }
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                if !*rx.borrow_and_update() {
                    break;
                }
                beat();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const TICK: Duration = Duration::from_secs(1);

    #[tokio::test(start_paused = true)]
    async fn beats_only_while_backfilling() {
        let hb = Arc::new(Heartbeat::default());
        let beats = Arc::new(AtomicUsize::new(0));
        let (h, b) = (hb.clone(), beats.clone());
        let task = tokio::spawn(async move {
            h.run(TICK, || {
                b.fetch_add(1, Ordering::SeqCst);
            })
            .await
        });
        let count = || beats.load(Ordering::SeqCst);

        // An idle hour: no beats (the old ticker woke 3,600 times).
        hb.phase(SyncPhase::Idle);
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert_eq!(count(), 0);

        // Ten seconds of backfill: a beat at once, then one a second.
        hb.phase(SyncPhase::Backfilling);
        tokio::time::sleep(Duration::from_millis(9_500)).await;
        assert_eq!(count(), 10);

        // Other phases don't restart or stop it; leaving Backfilling stops it.
        hb.phase(SyncPhase::Backfilling);
        hb.phase(SyncPhase::Idle);
        let at_stop = count();
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert_eq!(count(), at_stop);

        // And a later backfill starts it again.
        hb.phase(SyncPhase::Backfilling);
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(count(), at_stop + 3);
        task.abort();
    }
}
