//! User settings, stored as TOML in `$XDG_CONFIG_HOME/kantor/settings.toml`.
//!
//! A missing or corrupt file yields defaults (and is never overwritten until the
//! user changes a setting).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `omarchy` (follow the system), `omarchy:<name>` or `ghostty:<name>`.
    pub theme: String,
    /// UI font family; `None` picks the best installed one (SF Pro, Inter, …).
    pub ui_font: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "omarchy".to_owned(),
            ui_font: None,
        }
    }
}

/// `$XDG_CONFIG_HOME/kantor/settings.toml` (default `~/.config/kantor/settings.toml`).
pub fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kantor/settings.toml"))
}

impl Settings {
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn load() -> Self {
        settings_path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    /// Write atomically (temp file + rename).
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path().context("no home directory")?;
        self.save_to(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/settings.toml");
        assert_eq!(Settings::load_from(&path), Settings::default());

        let s = Settings {
            theme: "ghostty:Dracula".into(),
            ui_font: Some("SF Pro Text".into()),
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);

        std::fs::write(&path, "theme = [not valid").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());

        std::fs::write(&path, "theme = \"omarchy:nord\"\n").unwrap();
        assert_eq!(Settings::load_from(&path).theme, "omarchy:nord");
        assert_eq!(Settings::load_from(&path).ui_font, None);
    }
}
