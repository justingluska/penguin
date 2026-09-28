//! WAL checkpoints in the background.
//!
//! A checkpoint copies every page written since the last one from the WAL
//! back into the database file and syncs it. SQLite's automatic checkpoint
//! runs inside the commit that crosses its threshold, so that write, and
//! every write queued behind it, waited for it: during a backfill a third
//! of ingest time went to checkpoints at 4,000 pages, and at 16,384 single
//! writes stalled for up to ~400 ms (benchmark VM). Here a thread with its
//! own connection runs PASSIVE checkpoints, which copy pages while writes
//! go on, a moment after writes happen so that a page written again
//! meanwhile is copied once.
//!
//! SQLite starts the WAL over only when a write begins with every frame
//! already copied, which happens by itself whenever writes pause (sync
//! waits on the network between batches). Writes that never pause (index
//! maintenance, a local import) keep a passive checkpoint a little behind,
//! so the writer keeps its own automatic checkpoint as a backstop at
//! `WAL_BACKSTOP_PAGES`: by then this thread has copied nearly everything,
//! the commit that crosses it copies the rest (a fraction of a second's
//! writes), and the next write starts the WAL over. `journal_size_limit`
//! then cuts the file back.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::Connection;

use super::lock;
use crate::{Error, Result};

/// How long writes accumulate before a checkpoint copies them.
const CHECKPOINT_DELAY: Duration = Duration::from_millis(250);
/// The writer's own automatic checkpoint (see the module docs): 64 MiB.
pub(super) const WAL_BACKSTOP_PAGES: i64 = 16_384;

pub(super) struct Checkpointer {
    state: Mutex<State>,
    wake: Condvar,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct State {
    /// Written since the last checkpoint started.
    pending: bool,
    /// A checkpoint is waiting out its delay or running.
    busy: bool,
    stop: bool,
    /// Checkpoints run.
    runs: u64,
}

impl Checkpointer {
    pub(super) fn start(path: &Path) -> Result<Arc<Checkpointer>> {
        let conn = Connection::open(path)?;
        let me = Arc::new(Checkpointer {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            thread: Mutex::new(None),
        });
        let worker = me.clone();
        let handle = std::thread::Builder::new()
            .name("penguin-wal-checkpoint".into())
            .spawn(move || worker.run(&conn))
            .map_err(|e| Error::Db(format!("start the checkpoint thread: {e}")))?;
        *lock(&me.thread) = Some(handle);
        Ok(me)
    }

    /// The writer wrote (or may have): checkpoint soon.
    pub(super) fn written(&self) {
        let mut st = lock(&self.state);
        if !st.pending {
            st.pending = true;
            self.wake.notify_all();
        }
    }

    /// Checkpoints run so far, and whether there is nothing left to do.
    #[cfg(test)]
    pub(super) fn progress(&self) -> (u64, bool) {
        let st = lock(&self.state);
        (st.runs, !st.pending && !st.busy)
    }

    fn run(&self, conn: &Connection) {
        loop {
            {
                let mut st = lock(&self.state);
                while !st.pending && !st.stop {
                    st = self.wake.wait(st).unwrap_or_else(|e| e.into_inner());
                }
                st.busy = true;
                // Let more writes land first; only stopping cuts this short.
                let (mut st, _) = self
                    .wake
                    .wait_timeout_while(st, CHECKPOINT_DELAY, |s| !s.stop)
                    .unwrap_or_else(|e| e.into_inner());
                if st.stop {
                    return;
                }
                st.pending = false;
            }
            if let Err(e) = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(())) {
                tracing::warn!(error = %e, "WAL checkpoint failed");
            }
            let mut st = lock(&self.state);
            st.runs += 1;
            st.busy = false;
        }
    }

    /// Stop the thread and wait for it.
    pub(super) fn stop(&self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
        if let Some(handle) = lock(&self.thread).take() {
            let _ = handle.join();
        }
    }
}
