//! Security-scoped bookmarks through CoreFoundation's CFURL API.

#![allow(unsafe_code)]

use std::path::Path;
use std::ptr;
use std::sync::Mutex;

use objc2_core_foundation::{
    CFData, CFRetained, CFURL, CFURLBookmarkCreationOptions, CFURLBookmarkResolutionOptions,
};

use crate::store::{Opened, Os, usable};

pub struct System;

/// URLs whose access was started: kept (and so kept open) until Kantor quits.
struct Started(Vec<CFRetained<CFURL>>);

// SAFETY: CFURL is an immutable, reference-counted CoreFoundation object
// that may be used and released on any thread.
unsafe impl Send for Started {}

static STARTED: Mutex<Started> = Mutex::new(Started(Vec::new()));

impl Os for System {
    fn create(&self, path: &Path, dir: bool) -> Option<Vec<u8>> {
        let url = if dir {
            CFURL::from_directory_path(path)?
        } else {
            CFURL::from_file_path(path)?
        };
        // SAFETY: `url` is a valid CFURL, the optional arguments are null
        // (no resource properties, no relative URL, no error out-pointer).
        let data = unsafe {
            CFURL::new_bookmark_data(
                None,
                Some(&url),
                CFURLBookmarkCreationOptions::WithSecurityScope,
                None,
                None,
                ptr::null_mut(),
            )
        }?;
        Some(data.to_vec())
    }

    fn open(&self, data: &[u8]) -> Option<Opened> {
        let data = CFData::from_bytes(data);
        let mut stale: u8 = 0;
        // SAFETY: `data` is a valid CFData, `stale` a valid out-pointer for
        // a Boolean that lives across the call; no error out-pointer.
        let url = unsafe {
            CFURL::new_by_resolving_bookmark_data(
                None,
                Some(&data),
                CFURLBookmarkResolutionOptions::CFURLBookmarkResolutionWithSecurityScope,
                None,
                None,
                &mut stale,
                ptr::null_mut(),
            )
        }?;
        // SAFETY: `url` came from a security-scoped bookmark; the matching
        // stop is never called, the access lasts until Kantor quits.
        if !unsafe { url.start_accessing_security_scoped_resource() } {
            return None;
        }
        let path = url.to_file_path()?;
        STARTED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .push(url);
        Some(Opened {
            path: strip_trailing_slash(path),
            stale: stale != 0,
        })
    }

    fn can_use(&self, path: &Path) -> bool {
        usable(path)
    }
}

/// Folder URLs come back as `/a/b/`; Kantor's paths don't end in `/`.
fn strip_trailing_slash(path: std::path::PathBuf) -> std::path::PathBuf {
    path.components().collect()
}
