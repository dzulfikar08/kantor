//! Per-user directories that differ by platform.
//!
//! Linux and macOS keep config in `$XDG_CONFIG_HOME/kantor` and state in
//! `$XDG_STATE_HOME/kantor` (see [`crate::settings`], [`crate::session`]).
//! Windows uses the folders Windows apps are expected to:
//!
//! ```text
//! settings   %APPDATA%\Kantor            (roams with the user's profile)
//! state      %LOCALAPPDATA%\Kantor       (session, updater; this machine only)
//! ```
//!
//! Data that users may need to find (recovery copies) goes where each OS
//! expects it ([`data_dir`]).

use std::path::PathBuf;

/// An absolute directory from an environment variable.
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

#[cfg(target_os = "macos")]
fn platform_data_dir() -> Option<PathBuf> {
    Some(home_dir()?.join("Library/Application Support/Kantor"))
}

#[cfg(windows)]
fn platform_data_dir() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA")
        .or_else(|| env_dir("APPDATA"))
        .or_else(|| Some(home_dir()?.join("AppData").join("Local")))
        .map(|d| d.join("Kantor"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_data_dir() -> Option<PathBuf> {
    let base = env_dir("XDG_DATA_HOME").or_else(|| Some(home_dir()?.join(".local/share")))?;
    Some(base.join("kantor"))
}

/// Where Print puts the PDF it hands to the system's PDF viewer:
/// `<temp>/kantor-print-<pid>/<stem>.pdf` (the folder is created). The
/// viewer may still be reading it after Kantor quits, so it isn't deleted.
pub fn print_pdf_path(stem: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("kantor-print-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let stem: String = stem
        .chars()
        .map(|c| if std::path::is_separator(c) { '_' } else { c })
        .collect();
    let stem = if stem.trim().is_empty() {
        "Kantor"
    } else {
        stem.as_str()
    };
    Ok(dir.join(format!("{stem}.pdf")))
}
