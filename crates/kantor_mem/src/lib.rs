//! Giving freed heap memory back to the operating system.
//!
//! Dropping a big workbook frees hundreds of megabytes, but the C allocator
//! keeps the pages for later: on Linux, glibc returned almost nothing when a
//! 180k-row workbook was closed (258 MB resident before and after), and the
//! same holds, less badly, for the peaks of opening and recalculating. Call
//! [`release_free_memory`] after such moments, off the UI thread: it walks
//! the allocator's free lists, which takes a few milliseconds.
//!
//! The only unsafe code in Kantor: one call into the platform allocator,
//! which takes no pointers to Rust data and is thread-safe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Hands freed heap pages back to the operating system where the platform's
/// allocator supports it (glibc's `malloc_trim`, macOS's
/// `malloc_zone_pressure_relief`); does nothing elsewhere.
pub fn release_free_memory() {
    platform::release();
}

/// [`release_free_memory`] on a thread of its own after `delay`, at most one
/// such thread at a time (a call while one is waiting does nothing). For the
/// UI: after closing a document, say, when its memory has been freed.
pub fn release_free_memory_soon(delay: Duration) {
    static PENDING: AtomicBool = AtomicBool::new(false);
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("kantor-release-memory".into())
        .spawn(move || {
            std::thread::sleep(delay);
            PENDING.store(false, Ordering::Release);
            release_free_memory();
        });
    if spawned.is_err() {
        PENDING.store(false, Ordering::Release);
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
mod platform {
    unsafe extern "C" {
        /// glibc ≥ 2.8: releases free memory at the top of every arena's heap
        /// and the free pages inside them (`madvise`). Returns 1 if any.
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }

    pub(super) fn release() {
        // SAFETY: no arguments that point anywhere; glibc locks each arena.
        unsafe {
            malloc_trim(0);
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod platform {
    use std::ffi::c_void;

    unsafe extern "C" {
        /// macOS ≥ 10.7 (`<malloc/malloc.h>`): with no zone, every zone gives
        /// back up to `goal` bytes of free pages, 0 meaning all it can.
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }

    pub(super) fn release() {
        // SAFETY: a null zone means "all zones"; the zones lock themselves.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

#[cfg(not(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos")))]
mod platform {
    pub(super) fn release() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_is_harmless() {
        let big: Vec<Vec<u8>> = (0..1000).map(|i| vec![i as u8; 4096]).collect();
        drop(big);
        release_free_memory();
        release_free_memory_soon(Duration::ZERO);
        release_free_memory_soon(Duration::ZERO);
        let again = vec![7u8; 1 << 20];
        assert_eq!(again[12345], 7);
    }
}
