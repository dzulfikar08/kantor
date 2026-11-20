//! Secrets: in the OS keychain (macOS Keychain, Secret Service on Linux,
//! Windows Credential Manager), never in `settings.toml`, never logged.
//! Three [`Vault`]s share the code: AI provider keys ([`AI`]), cloud drive
//! app passwords ([`CLOUD`]) and e-meterai / e-signature provider
//! credentials ([`ESIGN`]).
//!
//! When no keychain is available (no Secret Service running, e.g. a bare
//! window manager without gnome-keyring/KWallet), secrets fall back to a
//! file next to the settings (`ai-keys.toml`, `cloud-keys.toml`), readable
//! only by the user (0600). [`Vault::save`] reports which one was used so
//! the UI can warn.
//!
//! These calls can block (D-Bus, an unlock prompt): run them off the UI
//! thread. Each waits a bounded time ([`CONNECT_WAIT`], [`CALL_WAIT`]); a
//! keychain that doesn't answer in time is left alone for the rest of the
//! run and the fallback file used instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// e-meterai and certified-signature provider credentials (kantor_esign),
/// keyed by provider id; each value is the provider's fields as JSON.
pub const ESIGN: Vault = Vault {
    service: "Kantor e-Sign",
    file: "esign-keys.toml",
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
            let (service, n) = (self.service, name.to_owned());
            match bounded("read", CALL_WAIT, move || {
                keyring_core::Entry::new(service, &n).and_then(|e| e.get_password())
            }) {
                Some(Ok(k)) => return Some(k),
                Some(Err(keyring_core::Error::NoEntry)) | None => {}
                Some(Err(e)) => log::warn!("keychain read failed: {e}"),
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
            let (service, n, s) = (self.service, name.to_owned(), secret.to_owned());
            match bounded("write", CALL_WAIT, move || {
                keyring_core::Entry::new(service, &n).and_then(|e| e.set_password(&s))
            }) {
                Some(Ok(())) => {
                    // Don't leave an older copy in the fallback file.
                    if let Some(p) = self.fallback_path() {
                        let _ = file_store(&p, name, None);
                    }
                    return Ok(KeyLocation::Keychain);
                }
                Some(Err(e)) => log::warn!("keychain unavailable, using the fallback file: {e}"),
                None => {}
            }
        }
        let path = self.fallback_path().ok_or("no home directory")?;
        file_store(&path, name, Some(secret))?;
        Ok(KeyLocation::File)
    }

    pub fn delete(&self, name: &str) {
        if keychain_ready() {
            let (service, n) = (self.service, name.to_owned());
            let _ = bounded("delete", CALL_WAIT, move || {
                keyring_core::Entry::new(service, &n).and_then(|e| e.delete_credential())
            });
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

/// How long connecting to the keychain may take. A Secret Service that
/// D-Bus is asked to start but that never comes up (measured 2026-11-16: a
/// second gnome-keyring-daemon in a session whose daemon belongs to another
/// bus) makes the call wait for D-Bus's 25 s activation timeout; after this
/// wait the fallback file is used for the rest of the run.
const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// How long one read, write or delete may take. It can wait for the user:
/// a locked keyring asks for its password, and the first secret on a system
/// without a keyring asks for a new keyring's password. After this wait the
/// fallback file is used for the rest of the run.
const CALL_WAIT: Duration = Duration::from_secs(60);

/// A keychain call took longer than its wait: don't use the keychain again
/// this run (the next call would wait just as long, or queue behind it).
static GAVE_UP: AtomicBool = AtomicBool::new(false);

/// Run a keychain call on a thread of its own and wait at most `wait` for
/// it (`None`: it took too long and is left running). Logs its time
/// (`RUST_LOG=kantor_ai=debug`).
fn bounded<T: Send + 'static>(
    what: &str,
    wait: Duration,
    call: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    if GAVE_UP.load(Ordering::Relaxed) {
        return None;
    }
    let start = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("kantor-keychain".into())
        .spawn(move || {
            let _ = tx.send(call());
        });
    if spawned.is_err() {
        return None;
    }
    match rx.recv_timeout(wait) {
        Ok(r) => {
            log::debug!("keychain {what}: {:.2} s", start.elapsed().as_secs_f64());
            Some(r)
        }
        Err(_) => {
            GAVE_UP.store(true, Ordering::Relaxed);
            log::warn!(
                "the keychain didn't answer a {what} within {} s: using the fallback file for the rest of this run",
                wait.as_secs()
            );
            None
        }
    }
}

/// Install the platform's credential store once; `false` if there is none
/// or it doesn't answer (see [`CONNECT_WAIT`]).
fn keychain_ready() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    let ready = *READY.get_or_init(|| match bounded("connect", CONNECT_WAIT, platform_store) {
        Some(Ok(store)) => {
            keyring_core::set_default_store(store);
            true
        }
        Some(Err(e)) => {
            log::warn!("no OS keychain: {e}");
            false
        }
        None => false,
    });
    ready && !GAVE_UP.load(Ordering::Relaxed)
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
    fn a_keychain_that_does_not_answer_is_given_up_on() {
        assert_eq!(bounded("test", Duration::from_secs(5), || 7), Some(7));
        let slow = bounded("test", Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(500));
        });
        assert_eq!(slow, None);
        // Later calls don't wait again this run.
        let start = Instant::now();
        assert_eq!(bounded("test", Duration::from_secs(5), || 7), None);
        assert!(start.elapsed() < Duration::from_secs(1));
        GAVE_UP.store(false, Ordering::Relaxed);
    }

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
