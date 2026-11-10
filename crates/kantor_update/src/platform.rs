//! How this copy of Kantor was installed, and which manifest asset fits it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// An install Kantor can replace itself in, or `Unmanaged` (package
/// manager, `cargo build`, a dev run): those only get a notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallKind {
    /// `…/Kantor.app` (the DMG or install.sh on macOS).
    MacApp {
        bundle: PathBuf,
    },
    /// The `.AppImage` file itself (`$APPIMAGE`).
    AppImage {
        path: PathBuf,
    },
    /// install.sh on Linux: `<prefix>/bin/kantor` plus launchers and icons
    /// under `<prefix>/share`.
    Tarball {
        prefix: PathBuf,
    },
    Unmanaged,
}

impl InstallKind {
    /// Detect from the running process.
    pub fn detect() -> Self {
        let Ok(exe) = std::env::current_exe() else {
            return Self::Unmanaged;
        };
        let exe = exe.canonicalize().unwrap_or(exe);
        let env = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        detect_from(&exe, env("APPIMAGE"), env("APPDIR"), std::env::consts::OS)
    }

    /// The manifest asset for this install on `arch` (`x86_64`, `aarch64`).
    pub fn asset_key(&self, arch: &str) -> Option<String> {
        let arch = match arch {
            "x86_64" | "aarch64" => arch,
            _ => return None,
        };
        match self {
            Self::MacApp { .. } => Some("macos-universal-app".to_owned()),
            Self::AppImage { .. } => Some(format!("linux-{arch}-appimage")),
            Self::Tarball { .. } => Some(format!("linux-{arch}-tarball")),
            Self::Unmanaged => None,
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Self::MacApp { .. } => "macOS app",
            Self::AppImage { .. } => "AppImage",
            Self::Tarball { .. } => "install.sh",
            Self::Unmanaged => "unmanaged",
        }
    }
}

/// [`InstallKind::detect`] with the inputs spelled out (for tests). `exe` is
/// the canonical path of the running binary.
pub fn detect_from(
    exe: &Path,
    appimage: Option<PathBuf>,
    appdir: Option<PathBuf>,
    os: &str,
) -> InstallKind {
    if os == "macos" {
        // …/Kantor.app/Contents/MacOS/kantor
        let bundle = exe.parent().and_then(Path::parent).and_then(Path::parent);
        let in_bundle = exe.parent().and_then(Path::file_name) == Some("MacOS".as_ref())
            && exe
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                == Some("Contents".as_ref());
        if let Some(bundle) = bundle
            && in_bundle
            && bundle.extension() == Some("app".as_ref())
        {
            return InstallKind::MacApp {
                bundle: bundle.to_path_buf(),
            };
        }
        return InstallKind::Unmanaged;
    }
    if os != "linux" {
        return InstallKind::Unmanaged;
    }
    // An AppImage runtime sets both and runs the binary from inside APPDIR; a
    // stray APPIMAGE inherited from another app doesn't count.
    if let (Some(image), Some(dir)) = (appimage, appdir)
        && exe.starts_with(&dir)
        && image.is_file()
    {
        return InstallKind::AppImage { path: image };
    }
    // install.sh: <prefix>/bin/kantor and a launcher it rewrote to point there.
    if exe.file_name() == Some("kantor".as_ref())
        && let Some(bin) = exe.parent()
        && bin.file_name() == Some("bin".as_ref())
        && let Some(prefix) = bin.parent()
    {
        let desktop = prefix.join("share/applications/kantor.desktop");
        let exec = format!("Exec={}", exe.display());
        if std::fs::read_to_string(desktop)
            .is_ok_and(|d| d.lines().any(|l| l.trim_start().starts_with(&exec)))
        {
            return InstallKind::Tarball {
                prefix: prefix.to_path_buf(),
            };
        }
    }
    InstallKind::Unmanaged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_bundle() {
        let k = detect_from(
            Path::new("/Applications/Kantor.app/Contents/MacOS/kantor"),
            None,
            None,
            "macos",
        );
        assert_eq!(
            k,
            InstallKind::MacApp {
                bundle: "/Applications/Kantor.app".into()
            }
        );
        assert_eq!(
            k.asset_key("aarch64").as_deref(),
            Some("macos-universal-app")
        );
        assert_eq!(
            k.asset_key("x86_64").as_deref(),
            Some("macos-universal-app")
        );
        // cargo run on a Mac
        assert_eq!(
            detect_from(
                Path::new("/src/kantor/target/release/kantor"),
                None,
                None,
                "macos"
            ),
            InstallKind::Unmanaged
        );
        assert_eq!(
            detect_from(
                Path::new("/x/Kantor/Contents/MacOS/kantor"),
                None,
                None,
                "macos"
            ),
            InstallKind::Unmanaged
        );
    }

    #[test]
    fn appimage() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("Kantor-x86_64.AppImage");
        std::fs::write(&image, b"").unwrap();
        let exe = Path::new("/tmp/.mount_KantorAb12/usr/bin/kantor");
        let k = detect_from(
            exe,
            Some(image.clone()),
            Some("/tmp/.mount_KantorAb12".into()),
            "linux",
        );
        assert_eq!(
            k,
            InstallKind::AppImage {
                path: image.clone()
            }
        );
        assert_eq!(
            k.asset_key("aarch64").as_deref(),
            Some("linux-aarch64-appimage")
        );
        assert_eq!(k.asset_key("riscv64"), None);
        // APPIMAGE leaked from another AppImage's environment.
        let other = detect_from(
            Path::new("/usr/bin/kantor"),
            Some(image),
            Some("/tmp/.mount_Other".into()),
            "linux",
        );
        assert_eq!(other, InstallKind::Unmanaged);
    }

    #[test]
    fn tarball_and_package() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join(".local");
        let exe = prefix.join("bin/kantor");
        std::fs::create_dir_all(prefix.join("share/applications")).unwrap();
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let desktop = prefix.join("share/applications/kantor.desktop");
        // A package's launcher says `Exec=kantor`: not ours to replace.
        std::fs::write(&desktop, "[Desktop Entry]\nExec=kantor %F\n").unwrap();
        assert_eq!(
            detect_from(&exe, None, None, "linux"),
            InstallKind::Unmanaged
        );
        std::fs::write(
            &desktop,
            format!("[Desktop Entry]\nExec={} %F\n", exe.display()),
        )
        .unwrap();
        let k = detect_from(&exe, None, None, "linux");
        assert_eq!(
            k,
            InstallKind::Tarball {
                prefix: prefix.clone()
            }
        );
        assert_eq!(
            k.asset_key("x86_64").as_deref(),
            Some("linux-x86_64-tarball")
        );
        assert_eq!(
            detect_from(Path::new("/usr/bin/kantor"), None, None, "linux"),
            InstallKind::Unmanaged
        );
        assert_eq!(InstallKind::Unmanaged.asset_key("x86_64"), None);
        assert_eq!(
            detect_from(&exe, None, None, "windows"),
            InstallKind::Unmanaged
        );
    }
}
