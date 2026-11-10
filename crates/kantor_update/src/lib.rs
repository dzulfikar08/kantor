//! In-app updates for direct-download installs (no GPUI).
//!
//! ```text
//! check   GET <channel>.json and .sig → Ed25519 verify → newer than this build?
//!         → pick the asset for this OS, arch and install kind
//! stage   download beside the install → size + SHA-256 → unpack → codesign,
//!         spctl (macOS) → run the new binary's --version
//! apply   rename the new build over the old one (all or nothing), at quit
//!         or on "Restart to update"; then relaunch
//! ```
//!
//! A check sends nothing but the User-Agent (`Kantor/<version> (<os>; <arch>)`).
//! Installs Kantor can't replace itself in (packages, `cargo build`) only get
//! a notice. See README "Updates".

pub mod http;
pub mod install;
pub mod manifest;
pub mod platform;
pub mod state;
pub mod verify;
pub mod version;

use std::fmt;
use std::path::PathBuf;

pub use install::{Staged, apply, discard, relaunch};
pub use manifest::{Asset, Manifest};
pub use platform::InstallKind;
pub use state::State;
pub use version::BuildId;

use http::Http;
use verify::Verifier;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Network(String),
    /// The manifest's signature doesn't verify with Kantor's key.
    Signature,
    Manifest(String),
    /// A download doesn't match the size or SHA-256 the manifest promised.
    Checksum(String),
    /// Kantor can't write where it is installed; download it by hand.
    NotWritable(PathBuf),
    /// The new build failed a signature, notarisation or start-up check.
    Verify(String),
    /// Installed by a package manager or built from source.
    Unmanaged,
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(e) => write!(f, "couldn't reach the update server ({e})"),
            Self::Signature => f.write_str("the update manifest's signature is not valid"),
            Self::Manifest(e) => write!(f, "the update manifest is invalid ({e})"),
            Self::Checksum(e) => write!(f, "the download is corrupt ({e})"),
            Self::NotWritable(dir) => write!(f, "Kantor can't write to {}", dir.display()),
            Self::Verify(e) => f.write_str(e),
            Self::Unmanaged => {
                f.write_str("this copy of Kantor is managed elsewhere (package or source build)")
            }
            Self::Io(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nightly,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "stable" => Some(Self::Stable),
            "nightly" => Some(Self::Nightly),
            _ => None,
        }
    }

    /// The channel this binary was built for: CI sets `KANTOR_CHANNEL=nightly`
    /// for nightly builds; everything else is stable.
    pub fn of_build() -> Self {
        match option_env!("KANTOR_CHANNEL") {
            Some("nightly") => Self::Nightly,
            _ => Self::Stable,
        }
    }

    /// The setting (`stable`, `nightly`), else the build's channel.
    pub fn from_setting(setting: Option<&str>) -> Self {
        setting.and_then(Self::parse).unwrap_or_else(Self::of_build)
    }
}

/// This build: the crate version plus the nightly stamp CI sets in
/// `KANTOR_BUILD` (UTC commit time, `YYYYMMDDHHMMSS`).
pub fn current_build() -> BuildId {
    let build = option_env!("KANTOR_BUILD")
        .and_then(|b| b.trim().parse().ok())
        .unwrap_or(0);
    BuildId::new(env!("CARGO_PKG_VERSION"), build).expect("the crate version is semver")
}

/// The build updates are compared against: [`current_build`], unless
/// `KANTOR_UPDATE_CURRENT` (testing only, e.g. `0.1.0`) pretends to be an
/// older version. It changes nothing else: manifests must still be signed with
/// Kantor's key and downloads must still pass every check. Honoured only in
/// nightly and debug builds: in a release it would let a replayed, older
/// signed manifest downgrade the app.
pub fn running_build() -> BuildId {
    if !cfg!(debug_assertions) && Channel::of_build() == Channel::Stable {
        return current_build();
    }
    match std::env::var("KANTOR_UPDATE_CURRENT") {
        Ok(v) if !v.trim().is_empty() => match BuildId::new(v.trim(), 0) {
            Ok(b) => {
                log::warn!("update: KANTOR_UPDATE_CURRENT: comparing as version {v}");
                b
            }
            Err(e) => {
                log::warn!("update: ignoring KANTOR_UPDATE_CURRENT: {e}");
                current_build()
            }
        },
        _ => current_build(),
    }
}

/// `https://kantor.paparanai.id/update/<channel>.json`; `KANTOR_UPDATE_URL`
/// replaces it (for testing against a local server).
pub fn manifest_url(channel: Channel) -> String {
    match std::env::var("KANTOR_UPDATE_URL") {
        Ok(url) if !url.trim().is_empty() => url.trim().to_owned(),
        _ => format!(
            "https://kantor.paparanai.id/update/{}.json",
            channel.as_str()
        ),
    }
}

/// What a check works from.
#[derive(Debug, Clone)]
pub struct Config {
    pub channel: Channel,
    pub manifest_url: String,
    pub public_key: [u8; 32],
    pub current: BuildId,
    pub kind: InstallKind,
    pub arch: String,
}

impl Config {
    /// For this process: its build, install kind and architecture.
    pub fn for_this_build(channel: Channel) -> Self {
        Self {
            channel,
            manifest_url: manifest_url(channel),
            public_key: manifest::decode_public_key(manifest::PUBLIC_KEY)
                .expect("the embedded update key is valid"),
            current: running_build(),
            kind: InstallKind::detect(),
            arch: std::env::consts::ARCH.to_owned(),
        }
    }
}

/// A newer build than this one.
#[derive(Debug, Clone)]
pub struct Available {
    pub manifest: Manifest,
    pub build: BuildId,
    /// The download for this install; `None`: tell the user, don't install
    /// (unmanaged install, or no build for this platform).
    pub asset: Option<Asset>,
}

impl Available {
    pub fn display_version(&self) -> String {
        self.build.display()
    }
}

/// Fetch and verify the manifest; `None` when this build is current.
pub fn check(cfg: &Config, http: &Http) -> Result<Option<Available>, Error> {
    let bytes = http.get(&cfg.manifest_url, manifest::MAX_MANIFEST)?;
    let sig = http.get(&format!("{}.sig", cfg.manifest_url), 1024)?;
    evaluate(cfg, &bytes, &sig)
}

/// [`check`] after the fetch.
pub fn evaluate(cfg: &Config, bytes: &[u8], sig: &[u8]) -> Result<Option<Available>, Error> {
    let manifest = manifest::verify(bytes, sig, &cfg.public_key)?;
    if manifest.channel != cfg.channel.as_str() {
        return Err(Error::Manifest(format!(
            "it is for the {} channel, not {}",
            manifest.channel,
            cfg.channel.as_str()
        )));
    }
    let build = manifest.build_id()?;
    if build <= cfg.current {
        return Ok(None);
    }
    let asset = cfg
        .kind
        .asset_key(&cfg.arch)
        .and_then(|key| manifest.assets.get(&key).cloned());
    Ok(Some(Available {
        manifest,
        build,
        asset,
    }))
}

/// Download and verify `available` for `cfg.kind`; see [`install::stage`].
pub fn stage(
    cfg: &Config,
    available: &Available,
    http: &Http,
    verifier: &dyn Verifier,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Staged, Error> {
    let asset = available.asset.as_ref().ok_or(Error::Unmanaged)?;
    install::stage(
        &available.manifest,
        asset,
        &cfg.kind,
        http,
        verifier,
        progress,
    )
}

/// A staged update is still worth applying: newer than this build, and its
/// files are still there.
pub fn staged_is_current(staged: &Staged, current: &BuildId) -> bool {
    BuildId::new(&staged.version, staged.build).is_ok_and(|b| &b > current)
        && staged
            .swaps
            .iter()
            .all(|(new, _)| std::fs::symlink_metadata(new).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{keypair, public, sign};

    fn cfg(kind: InstallKind, arch: &str, current: &str, build: u64) -> Config {
        Config {
            channel: Channel::Stable,
            manifest_url: String::new(),
            public_key: public(&keypair(1)),
            current: BuildId::new(current, build).unwrap(),
            kind,
            arch: arch.into(),
        }
    }

    fn manifest(channel: &str, version: &str, build: u64) -> Vec<u8> {
        let asset = |name: &str| {
            format!(
                r#""{name}": {{ "url": "https://example.com/{name}", "sha256": "{}", "size": 10 }}"#,
                "ab".repeat(32)
            )
        };
        format!(
            r#"{{ "channel": "{channel}", "version": "{version}", "build": {build},
  "date": "2026-11-10T00:00:00Z", "notes_url": "https://example.com/notes",
  "assets": {{ {}, {}, {}, {} }} }}"#,
            asset("macos-universal-app"),
            asset("linux-x86_64-appimage"),
            asset("linux-aarch64-appimage"),
            asset("linux-x86_64-tarball"),
        )
        .into_bytes()
    }

    #[test]
    fn picks_the_asset_for_the_install() {
        let m = manifest("stable", "0.2.1", 0);
        let sig = sign(&keypair(1), &m);
        let pick = |kind, arch| {
            evaluate(&cfg(kind, arch, "0.2.0", 0), &m, &sig)
                .unwrap()
                .unwrap()
                .asset
                .map(|a| a.url)
        };
        let image = InstallKind::AppImage {
            path: "/a/K.AppImage".into(),
        };
        let mac = InstallKind::MacApp {
            bundle: "/Applications/Kantor.app".into(),
        };
        let tar = InstallKind::Tarball {
            prefix: "/home/u/.local".into(),
        };
        assert_eq!(
            pick(image.clone(), "x86_64").as_deref(),
            Some("https://example.com/linux-x86_64-appimage")
        );
        assert_eq!(
            pick(image, "aarch64").as_deref(),
            Some("https://example.com/linux-aarch64-appimage")
        );
        assert_eq!(
            pick(mac, "aarch64").as_deref(),
            Some("https://example.com/macos-universal-app")
        );
        assert_eq!(
            pick(tar.clone(), "x86_64").as_deref(),
            Some("https://example.com/linux-x86_64-tarball")
        );
        // Not in this manifest: offered, but not installable.
        assert_eq!(pick(tar, "aarch64"), None);
        assert_eq!(pick(InstallKind::Unmanaged, "x86_64"), None);
    }

    #[test]
    fn newer_only() {
        let kind = InstallKind::Unmanaged;
        let run = |current: &str, build: u64, m: &[u8]| {
            evaluate(
                &cfg(kind.clone(), "x86_64", current, build),
                m,
                &sign(&keypair(1), m),
            )
        };
        let m = manifest("stable", "0.2.1", 0);
        assert!(run("0.2.0", 0, &m).unwrap().is_some());
        assert!(run("0.2.1", 0, &m).unwrap().is_none(), "same version");
        assert!(run("0.3.0", 0, &m).unwrap().is_none(), "never a downgrade");
        assert!(run("0.2.1-rc.1", 0, &m).unwrap().is_some());
        // A nightly of 0.2.1 isn't offered the 0.2.1 release.
        assert!(run("0.2.1", 20261111000000, &m).unwrap().is_none());
    }

    #[test]
    fn nightly_channel() {
        let m = manifest("nightly", "0.2.0", 20261111090000);
        let sig = sign(&keypair(1), &m);
        let mut c = cfg(InstallKind::Unmanaged, "x86_64", "0.2.0", 20261110090000);
        c.channel = Channel::Nightly;
        let a = evaluate(&c, &m, &sig).unwrap().unwrap();
        assert_eq!(a.display_version(), "0.2.0 (nightly 2026-11-11 09:00)");
        c.current = BuildId::new("0.2.0", 20261111090000).unwrap();
        assert!(evaluate(&c, &m, &sig).unwrap().is_none(), "this very build");
        // A nightly manifest served for the stable channel is refused.
        c.channel = Channel::Stable;
        assert!(matches!(evaluate(&c, &m, &sig), Err(Error::Manifest(_))));
    }

    #[test]
    fn channels() {
        assert_eq!(Channel::from_setting(Some("nightly")), Channel::Nightly);
        assert_eq!(Channel::from_setting(Some("stable")), Channel::Stable);
        assert_eq!(Channel::from_setting(Some("beta")), Channel::of_build());
        assert_eq!(Channel::from_setting(None), Channel::of_build());
        assert_eq!(
            current_build().version.to_string(),
            env!("CARGO_PKG_VERSION")
        );
    }
}
