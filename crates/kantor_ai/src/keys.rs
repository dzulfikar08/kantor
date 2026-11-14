//! Secrets: in the OS keychain (macOS Keychain, Secret Service on Linux,
//! Windows Credential Manager), never in `settings.toml`, never logged.
//! Two [`Vault`]s share the code: AI provider keys ([`AI`]) and cloud drive
//! app passwords ([`CLOUD`]).
//!
//! When no keychain is available (no Secret Service running, e.g. a bare
//! window manager without gnome-keyring/KWallet), secrets fall back to a
//! file next to the settings (`ai-keys.toml`, `cloud-keys.toml`), readable
//! only by the user (0600). [`Vault::save`] reports which one was used so
//! the UI can warn.
//!
//! These calls can block (D-Bus, an unlock prompt): run them off the UI thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A set of secrets under one keychain service name, with its own 0600
/// fallback file.
#[derive(Clone, Copy, Debug)]
pub struct Vault {
    service: &'static str,
    file: &'static str,
}

/// API keys of AI providers, keyed by provider (`anthropic`).
pub const AI: Vault = Vault {
    service: "Kantor AI",
    file: "ai-keys.toml",
};

/// Cloud drive app passwords, keyed by account id.
pub const CLOUD: Vault = Vault {
    service: "Kantor Cloud",
    file: "cloud-keys.toml",
};

/// Where a key ended up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyLocation {
    Keychain,
    /// The 0600 fallback file (no keychain available).
    File,
}

/// The stored key for `provider` (a provider key like `anthropic`).
pub fn load_key(provider: &str) -> Option<String> {
    AI.load(provider)
}

/// Store `key` for `provider`; an empty key deletes it.
pub fn save_key(provider: &str, key: &str) -> Result<KeyLocation, String> {
    AI.save(provider, key)
}

pub fn delete_key(provider: &str) {
    AI.delete(provider)
}

impl Vault {
    /// The stored secret for `name`.
    pub fn load(&self, name: &str) -> Option<String> {
        if keychain_ready() {
            match keyring_core::Entry::new(self.service, name).and_then(|e| e.get_password()) {
                Ok(k) => return Some(k),
                Err(keyring_core::Error::NoEntry) => {}
                Err(e) => log::warn!("keychain read failed: {e}"),
            }
        }
        self.fallback_path().and_then(|p| file_load(&p, name))
    }

    /// Store `secret` for `name`; an empty one deletes it.
    pub fn save(&self, name: &str, secret: &str) -> Result<KeyLocation, String> {
        let secret = secret.trim();
        if secret.is_empty() {
            self.delete(name);
            return Ok(KeyLocation::Keychain);
        }
        if keychain_ready() {
            match keyring_core::Entry::new(self.service, name).and_then(|e| e.set_password(secret))
            {
                Ok(()) => {
                    // Don't leave an older copy in the fallback file.
                    if let Some(p) = self.fallback_path() {
                        let _ = file_store(&p, name, None);
                    }
                    return Ok(KeyLocation::Keychain);
                }
                Err(e) => log::warn!("keychain unavailable, using the fallback file: {e}"),
            }
        }
        let path = self.fallback_path().ok_or("no home directory")?;
        file_store(&path, name, Some(secret))?;
        Ok(KeyLocation::File)
    }

    pub fn delete(&self, name: &str) {
        if keychain_ready()
            && let Ok(e) = keyring_core::Entry::new(self.service, name)
        {
            let _ = e.delete_credential();
        }
        if let Some(p) = self.fallback_path() {
            let _ = file_store(&p, name, None);
        }
    }

    /// The fallback file's path (for messages).
    pub fn fallback_path(&self) -> Option<PathBuf> {
        let settings = kantor_core::settings::settings_path()?;
        Some(settings.with_file_name(self.file))
    }
}

/// Install the platform's credential store once; `false` if there is none.
fn keychain_ready() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| match platform_store() {
        Ok(store) => {
            keyring_core::set_default_store(store);
            true
        }
        Err(e) => {
            log::warn!("no OS keychain: {e}");
            false
        }
    })
}

#[cfg(target_os = "linux")]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    zbus_secret_service_keyring_store::Store::new()
        .map(|s| s as std::sync::Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    apple_native_keyring_store::keychain::Store::new()
        .map(|s| s as std::sync::Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    windows_native_keyring_store::Store::new()
        .map(|s| s as std::sync::Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    Err("no keychain support on this platform".into())
}

fn file_load(path: &Path, provider: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let keys: BTreeMap<String, String> = toml::from_str(&text).ok()?;
    keys.get(provider).cloned()
}

/// Set (`Some`) or remove (`None`) one key in the fallback file, written
/// atomically with mode 0600.
fn file_store(path: &Path, provider: &str, key: Option<&str>) -> Result<(), String> {
    let mut keys: BTreeMap<String, String> = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    match key {
        Some(k) => {
            keys.insert(provider.to_owned(), k.to_owned());
        }
        None => {
            if keys.remove(provider).is_none() {
                return Ok(());
            }
        }
    }
    if keys.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string(&keys).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    write_private(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    // `mode` only applies to new files; tighten an existing one too.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_file_round_trip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kantor/ai-keys.toml");
        assert_eq!(file_load(&path, "openai"), None);
        file_store(&path, "openai", Some("sk-a")).unwrap();
        file_store(&path, "groq", Some("gsk-b")).unwrap();
        assert_eq!(file_load(&path, "openai").as_deref(), Some("sk-a"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        file_store(&path, "openai", None).unwrap();
        assert_eq!(file_load(&path, "openai"), None);
        assert_eq!(file_load(&path, "groq").as_deref(), Some("gsk-b"));
        file_store(&path, "groq", None).unwrap();
        assert!(!path.exists(), "empty file removed");
    }
}
