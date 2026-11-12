//! What the checker remembers between runs, in
//! `$XDG_STATE_HOME/kantor/update.json`: when it last checked, and a staged
//! update waiting for a restart.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::install::Staged;

/// Automatic checks run at most this often.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Unix seconds of the last automatic or manual check.
    pub last_check: Option<u64>,
    pub staged: Option<Staged>,
}

/// An automatic check is due: never checked, a day has passed, or the clock
/// went backwards (so a wrong clock can't stop checks for good).
pub fn due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(last) => now < last || now - last >= CHECK_INTERVAL_SECS,
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn state_path() -> Option<PathBuf> {
    Some(kantor_core::session::state_dir()?.join("update.json"))
}

impl State {
    pub fn load_from(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn load() -> Self {
        state_path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }

    pub fn save(&self) {
        let Some(path) = state_path() else {
            return;
        };
        if let Err(e) = self.save_to(&path) {
            log::warn!("saving {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::InstallKind;

    #[test]
    fn throttle() {
        let day = CHECK_INTERVAL_SECS;
        assert!(due(None, 1_000));
        assert!(!due(Some(1_000), 1_000));
        assert!(!due(Some(1_000), 1_000 + day - 1));
        assert!(due(Some(1_000), 1_000 + day));
        assert!(due(Some(1_000 + day * 10), 1_000), "clock went backwards");
    }

    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s/update.json");
        assert_eq!(State::load_from(&path), State::default());
        let s = State {
            last_check: Some(42),
            staged: Some(Staged {
                version: "0.2.1".into(),
                build: 0,
                notes_url: "https://x".into(),
                kind: InstallKind::AppImage {
                    path: "/a/Kantor.AppImage".into(),
                },
                swaps: vec![(
                    "/a/.Kantor.AppImage.update".into(),
                    "/a/Kantor.AppImage".into(),
                )],
                work_dir: None,
                installer: None,
            }),
        };
        s.save_to(&path).unwrap();
        assert_eq!(State::load_from(&path), s);
        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(State::load_from(&path), State::default());
    }
}
