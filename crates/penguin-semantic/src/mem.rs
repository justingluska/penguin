//! Give freed heap memory back to the OS.
//!
//! Loading the tokenizer parses a 20 MB JSON file into about 100 MB of
//! vocabulary and merge tables; the parse itself peaks near 290 MB, and
//! the allocator keeps what it freed. On Linux (glibc) that measured 288 MB
//! resident right after loading, 96 MB after `malloc_trim` (docs/SEMANTIC.md,
//! "Memory"). Dropping the model after idle leaves the same kind of freed,
//! still-resident heap. Calling this after either returns it.
//!
//! - glibc: `malloc_trim(0)` releases free memory at the top of the heap
//!   and, since glibc 2.8, free pages inside it (man 3 malloc_trim).
//! - macOS: `malloc_zone_pressure_relief(NULL, 0)` asks every malloc zone
//!   to release as much free memory as it can (<malloc/malloc.h>).
//! - Elsewhere: nothing.

/// Return freed heap pages to the OS. Cheap enough to call after a load or
/// an unload; not meant for hot paths.
pub fn release_freed() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: glibc call with no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means all zones; a goal of 0 means as much as
        // possible. No other preconditions.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}
