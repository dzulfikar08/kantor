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
    /// Cloud drives (`[cloud]`): WebDAV accounts. Passwords are never stored
    /// here: they live in the OS keychain (see `kantor_ai::keys`).
    pub cloud: CloudSettings,
    /// Sheets (`[sheets]`).
    pub sheets: SheetsSettings,
    /// `[pdf]`.
    pub pdf: PdfSettings,
    /// Interface language: `en` or `id`; unset follows the system language
    /// (Indonesian → `id`, anything else `en`). `KANTOR_LANG` overrides it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Regional format for numbers: `id` (`1.234,5`) or `en` (`1,234.5`);
    /// unset follows the interface language, as Excel follows the system's
    /// regional settings. Only how numbers are shown and typed changes:
    /// files keep Excel's invariant format codes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
}

/// `[cloud]`: WebDAV accounts (Nextcloud, OpenCloud, any RFC 4918 server).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    pub accounts: Vec<CloudAccount>,
    /// The folder "Open from Cloud" showed last, per account id.
    pub last_folder: BTreeMap<String, String>,
}

/// One cloud account. The app password is in the keychain under `id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudAccount {
    /// Stable id: names the keychain entry and the cache folder.
    pub id: String,
    /// The server address as typed (`https://drive.example.go.id`).
    pub server: String,
    pub user: String,
    /// The WebDAV root of the user's files; derived from `server` and
    /// `user` when empty (`<server>/remote.php/dav/files/<user>/`, which
    /// Nextcloud, ownCloud and OpenCloud all serve).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dav_url: String,
    /// How it signs in: empty for an app password, `oidc` for the
    /// organisation's single sign-on (the refresh token is in the keychain
    /// under `id`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub auth: String,
    /// Single sign-on: the identity provider (`https://sso.example.go.id/realms/pemda`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub issuer: String,
    /// Single sign-on: the client id Kantor signs in as (default `kantor`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    /// The name to show when `user` is an id (single sign-on accounts:
    /// `budi`, while Nextcloud's id may be a hash).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl CloudAccount {
    /// The WebDAV root the client talks to.
    pub fn dav_root(&self) -> String {
        if !self.dav_url.trim().is_empty() {
            return self.dav_url.trim().to_owned();
        }
        let server = self.server.trim().trim_end_matches('/');
        if server.contains("/remote.php/") || server.contains("/dav/") {
            return format!("{server}/");
        }
        format!("{server}/remote.php/dav/files/{}/", self.user.trim())
    }

    /// Whether it signs in with the organisation's single sign-on.
    pub fn is_sso(&self) -> bool {
        self.auth == "oidc"
    }

    /// `budi @ drive.example.go.id`.
    pub fn label(&self) -> String {
        let host = self
            .server
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let host = host.split('/').next().unwrap_or(host);
        let who = if self.name.is_empty() {
            &self.user
        } else {
            &self.name
        };
        format!("{who} @ {host}")
    }
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
            cloud: CloudSettings::default(),
            sheets: SheetsSettings::default(),
            pdf: PdfSettings::default(),
            language: None,
            number_format: None,
        }
    }
}

/// `[sheets]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SheetsSettings {
    /// "Let formulas download from the internet": IMAGE and WEBSERVICE
    /// fetch the URLs a workbook names. Off, they show `#BLOCKED!` (IMAGE
    /// cells keep the pictures saved in the file) and nothing is
    /// downloaded. Workbooks opened from a file still ask first (once per
    /// file and session).
    pub web_formulas: bool,
    /// Custom Lists the user made (Excel's Custom Lists dialog), each its
    /// items separated by commas (`Low, Medium, High`): Sort ▸ Order and
    /// fills use them besides Excel's built-in day and month lists.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_lists: Vec<String>,
}

impl Default for SheetsSettings {
    fn default() -> Self {
        Self {
            web_formulas: true,
            custom_lists: Vec::new(),
        }
    }
}

/// `[pdf]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfSettings {
    /// "Let signature checks use the internet": the Signatures panel may
    /// offer to fetch missing issuer certificates and ask the CAs' OCSP
    /// responders and CRLs whether certificates were revoked. Even when
    /// on, nothing is fetched until the user clicks Check Online for a
    /// document. Off, signatures are checked only with what the file holds.
    pub online_signature_checks: bool,
    /// Sign with Certificate: the RFC 3161 time-stamp server (empty: none).
    /// Used only when the user ticks "Add a time stamp" and online
    /// signature checks are allowed.
    pub signing_tsa_url: String,
    /// Sign with Certificate: tick "Add a time stamp" by default.
    pub signing_timestamp: bool,
    /// Sign with Certificate: tick "Add validation data (LTV)" by default.
    pub signing_ltv: bool,
    /// Sign with Certificate: the last reason and location.
    pub signing_reason: String,
    pub signing_location: String,
    /// Sign with Certificate: show the signature on the page (else invisible).
    pub signing_visible: bool,
    /// PKCS #11 modules (USB tokens, smart cards) to look for certificates
    /// in, besides the usual places.
    pub pkcs11_modules: Vec<String>,
    /// Certificate files (.p12/.pfx) used before. Never their passwords.
    pub certificate_files: Vec<String>,
    /// e-meterai and certified-signature providers set up in e-Sign
    /// Providers (`[[pdf.esign_providers]]`). Their credentials are in the
    /// system keychain, never here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub esign_providers: Vec<EsignProviderSettings>,
}

/// One e-meterai or certified-signature provider (`[[pdf.esign_providers]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EsignProviderSettings {
    /// The provider's id (kantor_esign's registry, e.g. `mekari`).
    pub id: String,
    /// The user allowed Kantor to send documents to this provider. Off,
    /// nothing goes to it, even when credentials are saved.
    pub allow_network: bool,
    /// Use the provider's sandbox (test) environment instead of production.
    pub sandbox: bool,
    /// Another server address (a provider's dedicated host, or a test
    /// server); empty: the provider's documented address.
    pub base_url: String,
    /// e-meterai: the document type used last (provider's code).
    pub doc_type: String,
}

impl Default for EsignProviderSettings {
    fn default() -> Self {
        Self {
            id: String::new(),
            allow_network: false,
            sandbox: true,
            base_url: String::new(),
            doc_type: String::new(),
        }
    }
}

impl Default for PdfSettings {
    fn default() -> Self {
        Self {
            online_signature_checks: true,
            signing_tsa_url: String::new(),
            signing_timestamp: false,
            signing_ltv: false,
            signing_reason: String::new(),
            signing_location: String::new(),
            signing_visible: true,
            pkcs11_modules: Vec::new(),
            certificate_files: Vec::new(),
            esign_providers: Vec::new(),
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
    fn web_formulas_are_on_unless_turned_off() {
        assert!(Settings::default().sheets.web_formulas);
        let off: Settings = toml::from_str("[sheets]\nweb_formulas = false\n").unwrap();
        assert!(!off.sheets.web_formulas);
        let other: Settings = toml::from_str("theme = \"omarchy\"\n").unwrap();
        assert!(other.sheets.web_formulas);
    }

    #[test]
    fn cloud_account_dav_root() {
        let mut a = CloudAccount {
            id: "a1".into(),
            server: "https://drive.example.go.id/".into(),
            user: "budi".into(),
            ..Default::default()
        };
        assert_eq!(
            a.dav_root(),
            "https://drive.example.go.id/remote.php/dav/files/budi/"
        );
        assert_eq!(a.label(), "budi @ drive.example.go.id");
        a.server = "https://x.example/remote.php/dav/files/budi".into();
        assert_eq!(a.dav_root(), "https://x.example/remote.php/dav/files/budi/");
        a.dav_url = "https://y.example/webdav/".into();
        assert_eq!(a.dav_root(), "https://y.example/webdav/");
    }

    #[test]
    fn cloud_settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut s = Settings::default();
        s.cloud.accounts.push(CloudAccount {
            id: "a1".into(),
            server: "https://d.example".into(),
            user: "sri".into(),
            ..Default::default()
        });
        // A single sign-on account: issuer and client, no secret.
        s.cloud.accounts.push(CloudAccount {
            id: "a2".into(),
            server: "https://d.example".into(),
            user: "0f3c9a".into(),
            auth: "oidc".into(),
            issuer: "https://sso.example/realms/pemda".into(),
            client_id: "kantor".into(),
            name: "budi".into(),
            ..Default::default()
        });
        s.cloud
            .last_folder
            .insert("a1".into(), "Keuangan/2026".into());
        s.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("password"));
        assert!(!text.contains("token"));
        let back = Settings::load_from(&path);
        assert_eq!(back, s);
        assert!(back.cloud.accounts[1].is_sso() && !back.cloud.accounts[0].is_sso());
        assert_eq!(back.cloud.accounts[1].label(), "budi @ d.example");
    }

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
            cloud: CloudSettings::default(),
            sheets: SheetsSettings {
                web_formulas: false,
                custom_lists: vec!["Low, Medium, High".into()],
            },
            pdf: PdfSettings {
                online_signature_checks: false,
                signing_tsa_url: "http://timestamp.example.com".into(),
                certificate_files: vec!["/home/x/me.p12".into()],
                esign_providers: vec![EsignProviderSettings {
                    id: "mekari".into(),
                    allow_network: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
            language: Some("id".into()),
            number_format: Some("en".into()),
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
