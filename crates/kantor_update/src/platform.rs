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
    /// Kantor-Setup-*.exe (Inno Setup, per user): `<dir>\kantor.exe` with
    /// the installer's `unins000.exe` beside it. Updated by running the new
    /// installer silently.
    WindowsInstaller {
        dir: PathBuf,
    },
    /// The portable Kantor-*.zip, unpacked anywhere: `<dir>\kantor.exe` with
    /// the zip's `kantor-portable.txt` beside it. Updated by swapping files.
    WindowsPortable {
        dir: PathBuf,
    },
    Unmanaged,
}

/// The file in the portable zip that marks its folder as Kantor's to update.
pub const PORTABLE_MARKER: &str = "kantor-portable.txt";

impl InstallKind {
    /// Detect from the running process.
    pub fn detect() -> Self {
        let Ok(exe) = std::env::current_exe() else {
            return Self::Unmanaged;
        };
        // Windows: current_exe is already absolute, and canonicalize would
        // turn it into a `\\?\` path that installers and shells dislike.
        let exe = if cfg!(windows) {
            exe
        } else {
            exe.canonicalize().unwrap_or(exe)
        };
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
            Self::WindowsInstaller { .. } => Some(format!("windows-{arch}-setup")),
            Self::WindowsPortable { .. } => Some(format!("windows-{arch}-portable")),
            Self::Unmanaged => None,
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Self::MacApp { .. } => "macOS app",
            Self::AppImage { .. } => "AppImage",
            Self::Tarball { .. } => "install.sh",
            Self::WindowsInstaller { .. } => "Windows installer",
            Self::WindowsPortable { .. } => "Windows portable",
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
    if os == "windows" {
        let is_kantor = exe
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("kantor.exe"));
        if let Some(dir) = exe.parent()
            && is_kantor
        {
            if dir.join("unins000.exe").is_file() {
                return InstallKind::WindowsInstaller {
                    dir: dir.to_path_buf(),
                };
            }
            if dir.join(PORTABLE_MARKER).is_file() {
                return InstallKind::WindowsPortable {
                    dir: dir.to_path_buf(),
                };
            }
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

    #[test]
    fn windows_installs() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Programs/Kantor");
        std::fs::create_dir_all(&app).unwrap();
        let exe = app.join("kantor.exe");
        std::fs::write(&exe, b"").unwrap();
        // cargo build output, or a copy with nothing beside it.
        assert_eq!(
            detect_from(&exe, None, None, "windows"),
            InstallKind::Unmanaged
        );
        std::fs::write(app.join(PORTABLE_MARKER), b"").unwrap();
        let k = detect_from(&exe, None, None, "windows");
        assert_eq!(k, InstallKind::WindowsPortable { dir: app.clone() });
        assert_eq!(
            k.asset_key("x86_64").as_deref(),
            Some("windows-x86_64-portable")
        );
        // The installer's uninstaller wins over a stray marker.
        std::fs::write(app.join("unins000.exe"), b"").unwrap();
        let k = detect_from(&app.join("Kantor.EXE"), None, None, "windows");
        assert_eq!(k, InstallKind::WindowsInstaller { dir: app.clone() });
        assert_eq!(
            k.asset_key("aarch64").as_deref(),
            Some("windows-aarch64-setup")
        );
        assert_eq!(
            detect_from(&app.join("other.exe"), None, None, "windows"),
            InstallKind::Unmanaged
        );
    }
}
