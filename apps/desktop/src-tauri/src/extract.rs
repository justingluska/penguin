//! Background extraction of structured facts for Ask (penguin-core
//! `structured`: flights, stays, orders, parcels, bills, bookings, contact
//! details). `Store::extract_pending` reads messages the extractors haven't
//! seen, newest first: the whole mailbox once, then whatever sync stores
//! (new mail, downloaded bodies). Each pass is short and yields to other
//! writers; nothing about a message is logged, only counts and timings.
//!
//! Once caught up it sleeps until local mail changes (`AppState::
//! mail_changes`) rather than polling: the 20-second poll it replaced woke
//! the process 180 times an hour to find nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::state::{background, AppState};

/// Messages per pass (one pass is a handful of short write transactions).
const PASS: usize = 400;
/// Pause between passes while there is a backlog, so sync and user actions
/// get the writer and the CPU.
const BETWEEN_PASSES: Duration = Duration::from_millis(150);
/// After a change wakes it, wait this long so a burst of sync commits is
/// read in one pass instead of one pass each.
const SETTLE: Duration = Duration::from_secs(2);
/// A failed pass is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);

pub fn spawn(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        // Subscribed before the startup pause: changes during it are seen.
        let mut changes = state.mail_changes();
        // Let startup sync and the first screen go first.
        tokio::time::sleep(Duration::from_secs(15)).await;
        let mut backlog: Option<(Instant, usize, usize)> = None; // (started, scanned, found)
        loop {
            // This pass covers every change so far; later ones wake the next.
            changes.borrow_and_update();
            let store = state.store.clone();
            let started = Instant::now();
            match background(move || Ok(store.extract_pending(PASS)?)).await {
                Ok(p) if p.scanned > 0 => {
                    let b = backlog.get_or_insert((started, 0, 0));
                    b.1 += p.scanned;
                    b.2 += p.found;
                    tracing::debug!(
                        scanned = p.scanned,
                        found = p.found,
                        remaining = p.remaining,
                        ms = started.elapsed().as_millis() as u64,
                        "extracted facts"
                    );
                    if p.remaining > 0 {
                        tokio::time::sleep(BETWEEN_PASSES).await;
                        continue;
                    }
                    if let Some((t0, scanned, found)) = backlog.take() {
                        let secs = t0.elapsed().as_secs_f64();
                        tracing::info!(
                            scanned,
                            found,
                            secs = format!("{secs:.1}"),
                            per_sec = format!("{:.0}", scanned as f64 / secs.max(0.001)),
                            "fact extraction caught up"
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e.message, "fact extraction failed; retrying later");
                    tokio::time::sleep(RETRY).await;
                    continue;
                }
            }
            // Caught up: sleep until local mail changes.
            if changes.changed().await.is_err() {
                return;
            }
            tokio::time::sleep(SETTLE).await;
        }
    });
}
