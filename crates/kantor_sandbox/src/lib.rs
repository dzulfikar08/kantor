//! File access inside the macOS App Sandbox, for the Mac App Store build
//! ([`kantor_core::SANDBOXED`]). Everywhere else every function here is a
//! no-op that reports access.
//!
//! A sandboxed app may use a file the user picked in an open or save
//! panel, opened from Finder or dropped on its window, until it quits.
//! To open it again in a later run (last session's tabs, recent files,
//! Batch folders, a mail-merge data source, a linked picture) it keeps an
//! app-scoped *security-scoped bookmark*, made while it still has access
//! ([`remember`]) and resolved on the next run ([`access`]). Bookmarks are
//! kept in `bookmarks.json` in the state folder (inside the container),
//! newest first, at most [`store::MAX`].
//!
//! A folder's bookmark covers everything in it, so writing next to a file
//! (`report (1).pdf`, a signed copy, a converted file) needs access to the
//! folder: [`access_dir_of`]. When Kantor has no grant, the UI asks the user
//! to pick the file or folder (an open panel on its folder: Word for Mac's
//! "Grant File Access") and calls [`remember`] with the answer.

pub mod store;

#[cfg(target_os = "macos")]
mod macos;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use store::{Os, Store};

/// Whether file access is restricted (the sandboxed store build).
pub const ACTIVE: bool = kantor_core::SANDBOXED;

/// Remember access to `path` (a file or folder Kantor can use now) for
/// later runs. Call it with what the user picked, opened or saved.
pub fn remember(path: &Path) {
    if ACTIVE && !in_container(path) {
        with_store(|s, os| s.remember(os, path));
    }
}

/// [`remember`] for each of `paths` that has no bookmark yet (cheap for
/// the ones that have, and for ones that failed earlier in this run).
pub fn remember_all<'a>(paths: impl IntoIterator<Item = &'a Path>) {
    if ACTIVE {
        with_store(|s, os| {
            for p in paths.into_iter().filter(|p| !in_container(p)) {
                s.remember_once(os, p);
            }
        });
    }
}

/// Make `path` usable for this run: `true` when Kantor may use it (always
/// outside the sandbox). Starts using the bookmark of `path` or of the
/// nearest folder above it that has one; with none, `true` only when the
/// sandbox already allows it (picked this run), and then it is remembered.
pub fn access(path: &Path) -> bool {
    if !ACTIVE || in_container(path) {
        return true;
    }
    with_store(|s, os| s.access(os, path))
}

/// Kantor's own container (settings, cloud working copies, recovery
/// copies) is always usable and needs no bookmark.
fn in_container(path: &Path) -> bool {
    kantor_core::dirs::home_dir().is_some_and(|home| path.starts_with(home))
}

/// [`access`] for the folder `path` is in (to create files next to it).
pub fn access_dir_of(path: &Path) -> bool {
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => access(dir),
        None => true,
    }
}

/// Where to point the "Grant File Access" panel for `path`: the nearest
/// folder above it that exists (as far as Kantor can tell).
pub fn grant_panel_dir(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|d| d.is_dir())
        .map(Path::to_path_buf)
}

fn with_store<T>(f: impl FnOnce(&mut Store, &dyn Os) -> T) -> T {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    let store = STORE.get_or_init(|| Mutex::new(Store::load(store_path())));
    let mut s = store.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut s, system())
}

fn store_path() -> Option<PathBuf> {
    Some(kantor_core::session::state_dir()?.join("bookmarks.json"))
}

#[cfg(target_os = "macos")]
fn system() -> &'static dyn Os {
    &macos::System
}

#[cfg(not(target_os = "macos"))]
fn system() -> &'static dyn Os {
    &store::NoOs
}
