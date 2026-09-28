//! Give freed heap back to the OS after a known burst of short-lived
//! allocations.
//!
//! Loading the search model parses a 20 MB tokenizer.json: on the build VM
//! the process's anonymous memory went from ~0 to 287 MB, of which only
//! 68 MB was still in use once loading returned (glibc's own count). The
//! allocator keeps the freed pages for reuse, so the process looked ~190 MB
//! bigger than it was until something else happened to reuse them; and
//! after the idle unload the whole model's pages stayed resident the same
//! way. Swapping the allocator doesn't fix it (measured, tokenizer loaded →
//! dropped: mimalloc 326 → 326 MB, jemalloc 253 → 203 MB); asking the
//! allocator to release what's free does (94 → 1 MB). Details and numbers:
//! docs/PERFORMANCE.md, "The app process".
//!
//! - macOS: `malloc_zone_pressure_relief(NULL, 0)` asks every malloc zone
//!   to return as much free memory as it can (`<malloc/malloc.h>`).
//! - Linux (glibc): `malloc_trim(0)` releases free pages anywhere in the
//!   heap, not only at its top (`man 3 malloc_trim`).

use std::time::Instant;

/// Release free heap pages to the OS. Costs a walk of the allocator's free
/// lists (milliseconds), so call it after a large, rare transient, not on
/// a hot path. Logged at info (it happens a few times a day at most), with
/// the bytes returned where the allocator reports them (macOS), so the Mac
/// numbers can be read from penguin.log.
pub fn release_free_heap(after: &'static str) {
    let started = Instant::now();
    let released = release();
    let ms = started.elapsed().as_millis() as u64;
    match released {
        Some(bytes) => tracing::info!(after, mb = bytes / 1_048_576, ms, "released free heap"),
        None => tracing::info!(after, ms, "released free heap"),
    }
}

/// Bytes returned, when the allocator says.
#[cfg(target_os = "macos")]
fn release() -> Option<usize> {
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    // SAFETY: a null zone means every zone; goal 0 means as much as possible.
    Some(unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) })
}

/// glibc only says whether anything was released.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release() -> Option<usize> {
    extern "C" {
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }
    // SAFETY: plain glibc call; it only returns free pages.
    unsafe {
        malloc_trim(0);
    }
    None
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
fn release() -> Option<usize> {
    None
}
