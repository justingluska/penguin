//! Quality of service for background work on macOS.
//!
//! Apple: "Optimally, run your app at a QoS level of utility or lower at
//! least 90% of the time when user activity is not occurring", and the
//! system "uses QoS information to adjust priorities such as scheduling,
//! CPU and I/O throughput, and timer latency" (Energy Efficiency Guide for
//! Mac Apps, "Prioritize Work at the Task Level"). Fact extraction reads the
//! whole mailbox once (about a minute of CPU per 100k messages on the build
//! VM) and the verification-code backfill a month of it; nobody waits on
//! either, so they run at utility QoS, on the blocking pool's threads,
//! which go back to their previous class afterwards so a command that runs
//! on the same thread later isn't slowed.
//!
//! These passes take the store's writer mutex for short transactions. std's
//! Mutex is a pthread mutex on macOS, and "when pthread_mutex_lock() is
//! called while the mutex is held by a thread with lower QoS … the thread
//! holding the lock is raised to the QoS of the caller" (same guide,
//! "Priority Inversion"), so a user action is never stuck behind them.
//! Elsewhere this is a no-op.

/// Run `f` at utility QoS on the current thread, then restore its class.
pub fn utility<T>(f: impl FnOnce() -> T) -> T {
    let _restore = Lowered::utility();
    f()
}

struct Lowered {
    #[cfg(target_os = "macos")]
    previous: (libc::qos_class_t, i32),
}

#[cfg(target_os = "macos")]
impl Lowered {
    fn utility() -> Self {
        use libc::qos_class_t::{QOS_CLASS_UNSPECIFIED, QOS_CLASS_UTILITY};
        let mut class = QOS_CLASS_UNSPECIFIED;
        let mut relative = 0;
        // SAFETY: plain libc calls on the current thread.
        unsafe {
            libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative);
            libc::pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
        Lowered {
            previous: (class, relative),
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Lowered {
    fn drop(&mut self) {
        use libc::qos_class_t::{QOS_CLASS_DEFAULT, QOS_CLASS_UNSPECIFIED};
        let (class, relative) = self.previous;
        // A thread nobody gave a class runs as default.
        let class = match class {
            QOS_CLASS_UNSPECIFIED => QOS_CLASS_DEFAULT,
            other => other,
        };
        // SAFETY: plain libc call on the current thread.
        unsafe {
            libc::pthread_set_qos_class_self_np(class, relative);
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl Lowered {
    fn utility() -> Self {
        Lowered {}
    }
}
