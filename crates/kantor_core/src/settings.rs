//! User settings, stored as TOML in `$XDG_CONFIG_HOME/kantor/settings.toml`.
//!
//! A missing or corrupt file yields defaults (and is never overwritten until the
//! user changes a setting).

use std::collections::BTreeMap;
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
    /// Show the ruler above Docs pages.
    pub docs_ruler: bool,
    /// AI assistant (provider, models). API keys are never stored here: they
    /// live in the OS keychain (see `kantor_ai::keys`).
    pub ai: AiSettings,
    /// In-app updates for direct-download installs (see `kantor_update`).
    pub update: UpdateSettings,
    /// Saving documents by itself (recovery copies are always on).
    pub autosave: AutosaveSettings,
    /// Name written on comments added in Docs (`author = "…"`); unset uses
    /// the account's full name, then the user name.
    pub author: Option<String>,
    /// Spell checking (`[spelling]`).
    pub spelling: SpellingSettings,
}

/// `[spelling]`: offline spell checking in Docs and Slides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellingSettings {
    /// "Check Spelling as You Type": red wavy underlines under unknown words.
    pub check_as_you_type: bool,
    /// Dictionary for text without a language of its own (`en_US` or
    /// `id_ID`); unset follows the system language (Indonesian → `id_ID`,
    /// anything else `en_US`). Text marked with a language in the document
    /// (`w:lang`) uses that language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Default for SpellingSettings {
    fn default() -> Self {
        Self {
            check_as_you_type: true,
            language: None,
        }
    }
}

impl Settings {
    /// The name for comments: `author`, else the account's name.
    pub fn author_name(&self) -> String {
        self.author
            .clone()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(crate::dirs::user_full_name)
    }
}

/// `[autosave]`: off by default. Recovery copies already protect against
/// crashes; saving over the user's file without being asked would also keep
/// changes they meant to throw away ("Don't Save"), so it is opt-in, as in
/// Office and LibreOffice for local files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutosaveSettings {
    /// Save documents that have a file in their own format (.xlsx, .docx,
    /// .pptx) after they've been left alone for `idle_seconds`. Imported files
    /// (.csv, .ods, .xls, .ppt) and files with content Kantor can't keep are never
    /// saved this way.
    pub enabled: bool,
    /// Seconds without changes before an autosave (at least 5).
    pub idle_seconds: u64,
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_seconds: 30,
        }
    }
}

impl AutosaveSettings {
    pub const MIN_IDLE_SECONDS: u64 = 5;

    /// The idle time, with the minimum applied.
    pub fn idle(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.idle_seconds.max(Self::MIN_IDLE_SECONDS))
    }
}

/// `[update]`: checks at most daily, sending only Kantor's version and OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Off: Kantor never contacts the update server, not even for
    /// "Check for Updates…".
    pub enabled: bool,
    /// `stable` or `nightly`; unset follows the build (nightly builds follow
    /// the nightly channel, releases the stable one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            channel: None,
        }
    }
}

/// AI settings. Everything is off until the user picks a provider and turns
/// the master switch on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Master switch: no request is ever sent while this is off.
    pub enabled: bool,
    /// The chosen provider's id (`anthropic`, `openai`, `ollama`, …).
    pub provider: String,
    /// Per-provider overrides, keyed by provider id.
    pub providers: BTreeMap<String, AiProviderSettings>,
}

/// What the user changed for one provider; `None` uses the provider default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiProviderSettings {
    pub model: Option<String>,
    pub base_url: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "omarchy".to_owned(),
            ui_font: None,
            docs_ruler: true,
            ai: AiSettings::default(),
            update: UpdateSettings::default(),
            autosave: AutosaveSettings::default(),
            author: None,
            spelling: SpellingSettings::default(),
        }
    }
}

/// `$XDG_CONFIG_HOME/kantor/settings.toml` (default `~/.config/kantor/settings.toml`);
/// on Windows `%APPDATA%\Kantor\settings.toml` (see [`crate::dirs`]).
pub fn settings_path() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.toml"))
}

/// Kantor's per-user config folder: `$XDG_CONFIG_HOME/kantor` (default
/// `~/.config/kantor`); on Windows `%APPDATA%\Kantor`. Holds settings.toml
/// and the personal spelling dictionary.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return crate::dirs::windows_config_dir();
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kantor"))
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
            docs_ruler: false,
            ai: AiSettings {
                enabled: true,
                provider: "ollama".into(),
                providers: BTreeMap::from([(
                    "ollama".into(),
                    AiProviderSettings {
                        model: Some("llama3.2".into()),
                        base_url: None,
                    },
                )]),
            },
            update: UpdateSettings {
                enabled: false,
                channel: Some("nightly".into()),
            },
            autosave: AutosaveSettings {
                enabled: true,
                idle_seconds: 60,
            },
            author: Some("Dewi Lestari".into()),
            spelling: SpellingSettings {
                check_as_you_type: false,
                language: Some("id_ID".into()),
            },
        };
        assert_eq!(s.author_name(), "Dewi Lestari");
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);

        std::fs::write(&path, "theme = [not valid").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());

        std::fs::write(&path, "theme = \"omarchy:nord\"\n").unwrap();
        assert_eq!(Settings::load_from(&path).theme, "omarchy:nord");
        assert_eq!(Settings::load_from(&path).ui_font, None);
        assert!(Settings::load_from(&path).docs_ruler, "ruler on by default");
        assert!(!Settings::load_from(&path).ai.enabled, "AI off by default");
        assert!(
            Settings::load_from(&path).update.enabled,
            "updates on by default"
        );

        assert!(
            !Settings::load_from(&path).autosave.enabled,
            "autosave off by default"
        );

        std::fs::write(&path, "[autosave]\nenabled = true\nidle_seconds = 1\n").unwrap();
        let s = Settings::load_from(&path);
        assert!(s.autosave.enabled);
        assert_eq!(
            s.autosave.idle().as_secs(),
            AutosaveSettings::MIN_IDLE_SECONDS
        );

        assert!(
            Settings::load_from(&path).spelling.check_as_you_type,
            "spell check on by default"
        );
        std::fs::write(&path, "[spelling]\nlanguage = \"en_US\"\n").unwrap();
        let s = Settings::load_from(&path);
        assert!(s.spelling.check_as_you_type);
        assert_eq!(s.spelling.language.as_deref(), Some("en_US"));

        std::fs::write(&path, "[update]\nenabled = false\n").unwrap();
        let s = Settings::load_from(&path);
        assert!(!s.update.enabled);
        assert_eq!(s.update.channel, None);
    }
}
