//! Per-user base directories that differ by OS.
//!
//! Linux and macOS keep using the XDG paths in [`crate::settings`] and
//! [`crate::session`]. Windows uses the folders Windows apps are expected to:
//!
//! ```text
//! settings   %APPDATA%\Kantor            (roams with the user's profile)
//! state      %LOCALAPPDATA%\Kantor       (session, updater; this machine only)
//! ```

use std::path::PathBuf;

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The user's home folder: `HOME`, else (Windows) `USERPROFILE`.
pub fn home_dir() -> Option<PathBuf> {
    env_dir("HOME").or_else(|| {
        if cfg!(windows) {
            env_dir("USERPROFILE")
        } else {
            None
        }
    })
}

/// Windows: `%APPDATA%\Kantor`, where settings.toml lives.
pub fn windows_config_dir() -> Option<PathBuf> {
    Some(env_dir("APPDATA")?.join("Kantor"))
}

/// Windows: `%LOCALAPPDATA%\Kantor`, for the session and the updater's state.
pub fn windows_state_dir() -> Option<PathBuf> {
    Some(env_dir("LOCALAPPDATA")?.join("Kantor"))
}
