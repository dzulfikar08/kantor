//! Per-user directories that differ by platform.
//!
//! Kantor keeps its config in `$XDG_CONFIG_HOME/kantor` and its state in
//! `$XDG_STATE_HOME/kantor` everywhere (see `settings`, `session`). Data that
//! users may need to find (recovery copies) goes where each OS expects it.

use std::path::PathBuf;

/// Kantor's per-user data directory:
///
/// | OS      | Directory                                       |
/// |---------|-------------------------------------------------|
/// | Linux   | `$XDG_DATA_HOME/kantor` (`~/.local/share/kantor`) |
/// | macOS   | `~/Library/Application Support/Kantor`          |
/// | Windows | `%LOCALAPPDATA%\Kantor` (else `%APPDATA%\Kantor`) |
///
/// `KANTOR_DATA_DIR` overrides it on every OS (tests, portable installs).
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = env_dir("KANTOR_DATA_DIR") {
        return Some(dir);
    }
    platform_data_dir()
}

/// An absolute directory from an environment variable.
fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn home() -> Option<PathBuf> {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE"))
}

#[cfg(target_os = "macos")]
fn platform_data_dir() -> Option<PathBuf> {
    Some(home()?.join("Library/Application Support/Kantor"))
}

#[cfg(windows)]
fn platform_data_dir() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA")
        .or_else(|| env_dir("APPDATA"))
        .or_else(|| Some(home()?.join("AppData").join("Local")))
        .map(|d| d.join("Kantor"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_data_dir() -> Option<PathBuf> {
    let base = env_dir("XDG_DATA_HOME").or_else(|| Some(home()?.join(".local/share")))?;
    Some(base.join("kantor"))
}
