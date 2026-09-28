//! Thread quality-of-service classes for background embedding (macOS).
//!
//! Apple: on Apple silicon "the system is more likely to run background
//! tasks on lower performance cores"; threads at QoS *background* run on
//! the efficiency cores only. Use `pthread_set_qos_class_self_np`, not
//! `setpriority` ("Tuning your code's performance for Apple silicon",
//! developer.apple.com). Each thread sets its own class. No-ops elsewhere.

/// Utility QoS: below anything the user is doing, still allowed on
/// performance cores. The indexer and ONNX Runtime's workers use it.
pub fn set_utility_qos() {
    #[cfg(target_os = "macos")]
    // SAFETY: plain libc call on the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// Background QoS: efficiency cores only.
pub fn set_background_qos() {
    #[cfg(target_os = "macos")]
    // SAFETY: plain libc call on the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
}
