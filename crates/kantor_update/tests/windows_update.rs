//! The Windows update paths (installer and portable zip) against a local HTTP
//! server and a fake signed manifest. They run on every OS: nothing here
//! starts a Windows program. The stand-in for the new kantor.exe is a text
//! file whose "version run" a fake verifier reads, and the silent installer
//! run is checked by its command line (packaging CI runs the real installer).

mod common;

use std::io::Write as _;
use std::path::Path;

use common::*;
use kantor_update::http::Http;
use kantor_update::verify::Verifier;
use kantor_update::{BuildId, Error, InstallKind};

/// `probe` reads what the stand-in kantor.exe would print: its contents.
struct ReadsVersion;

impl Verifier for ReadsVersion {
    fn check_app(&self, _: &Path) -> Result<(), Error> {
        unreachable!("macOS only")
    }
    fn probe(&self, program: &Path, args: &[&str]) -> Result<String, Error> {
        assert_eq!(args, ["--version"]);
        std::fs::read_to_string(program).map_err(|e| Error::Verify(e.to_string()))
    }
}

/// What scripts/package-windows.ps1 zips: files at the top, no folder.
fn portable_zip(version: &str) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, text) in [
            ("kantor.exe", format!("kantor {version}")),
            ("kantor.com", format!("console {version}")),
            ("kantor-portable.txt", "portable".to_owned()),
            ("LICENSE.txt", "AGPL".to_owned()),
        ] {
            z.start_file(name, opts).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn portable_zip_update() {
    let (base, routes) = serve();
    let url = publish(
        &base,
        &routes,
        "0.2.1",
        "windows-x86_64-portable",
        &portable_zip("0.2.1"),
    );
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("Kantor");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("kantor.exe"), "kantor 0.2.0").unwrap();
    std::fs::write(app.join("kantor.com"), "console 0.2.0").unwrap();
    std::fs::write(app.join("kantor-portable.txt"), "portable").unwrap();
    std::fs::write(app.join("my-notes.txt"), "keep").unwrap();
    let kind = kantor_update::platform::detect_from(&app.join("kantor.exe"), None, None, "windows");
    assert_eq!(kind, InstallKind::WindowsPortable { dir: app.clone() });

    let cfg = config(url, kind.clone());
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let staged =
        kantor_update::stage(&cfg, &available, &http, &ReadsVersion, &mut |_, _| {}).unwrap();
    assert_eq!(
        read(&app.join("kantor.exe")),
        "kantor 0.2.0",
        "untouched until apply"
    );
    assert!(staged.installer.is_none());
    // kantor.exe goes in last.
    assert!(staged.swaps.last().unwrap().1.ends_with("kantor.exe"));
    assert!(kantor_update::staged_is_current(&staged, &cfg.current));

    kantor_update::apply(&staged).unwrap();
    assert_eq!(read(&app.join("kantor.exe")), "kantor 0.2.1");
    assert_eq!(read(&app.join("kantor.com")), "console 0.2.1");
    assert_eq!(read(&app.join("LICENSE.txt")), "AGPL");
    assert_eq!(read(&app.join("my-notes.txt")), "keep");
    kantor_update::clean_leftovers(&kind, None);
    assert!(leftovers(&app).is_empty(), "{:?}", leftovers(&app));
    // Still a portable install afterwards.
    assert_eq!(
        kantor_update::platform::detect_from(&app.join("kantor.exe"), None, None, "windows"),
        kind
    );
}

#[test]
fn portable_zip_with_the_wrong_build_is_refused() {
    let (base, routes) = serve();
    let url = publish(
        &base,
        &routes,
        "0.2.1",
        "windows-x86_64-portable",
        &portable_zip("0.1.0"),
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("kantor.exe"), "kantor 0.2.0").unwrap();
    let cfg = config(
        url,
        InstallKind::WindowsPortable {
            dir: dir.path().to_path_buf(),
        },
    );
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let err =
        kantor_update::stage(&cfg, &available, &http, &ReadsVersion, &mut |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::Verify(_)), "{err:?}");
    assert_eq!(read(&dir.path().join("kantor.exe")), "kantor 0.2.0");
    assert!(leftovers(dir.path()).is_empty());
}

#[test]
fn installer_update_is_downloaded_and_checked() {
    let setup = b"MZ pretend this is Kantor-Setup-x64.exe".to_vec();
    let (base, routes) = serve();
    let url = publish(&base, &routes, "0.2.1", "windows-x86_64-setup", &setup);
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("Programs/Kantor");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("kantor.exe"), "kantor 0.2.0").unwrap();
    std::fs::write(app.join("unins000.exe"), "uninstaller").unwrap();
    let kind = kantor_update::platform::detect_from(&app.join("kantor.exe"), None, None, "windows");
    assert_eq!(kind, InstallKind::WindowsInstaller { dir: app.clone() });

    let cfg = config(url, kind.clone());
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let staged =
        kantor_update::stage(&cfg, &available, &http, &ReadsVersion, &mut |_, _| {}).unwrap();
    let installer = staged.installer.clone().expect("an installer to run");
    assert_eq!(std::fs::read(&installer).unwrap(), setup);
    assert!(staged.swaps.is_empty());
    assert!(installer.starts_with(&app));
    assert!(kantor_update::staged_is_current(&staged, &cfg.current));
    let cmd = kantor_update::install::installer_command(&installer, 1234, true);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"/VERYSILENT".to_owned()), "{args:?}");
    assert!(args.contains(&"/WAITPID=1234".to_owned()), "{args:?}");

    // Dropped (switched off, or older than the running build): nothing left.
    kantor_update::discard(&staged);
    assert!(!kantor_update::staged_is_current(&staged, &cfg.current));
    kantor_update::clean_leftovers(&kind, None);
    assert!(leftovers(&app).is_empty(), "{:?}", leftovers(&app));
    assert_eq!(read(&app.join("kantor.exe")), "kantor 0.2.0");
}

#[test]
fn corrupt_installer_is_not_staged() {
    let (base, routes) = serve();
    let url = publish(
        &base,
        &routes,
        "0.2.1",
        "windows-x86_64-setup",
        b"good setup",
    );
    routes
        .lock()
        .unwrap()
        .insert("/asset".into(), (200, b"evil setup".to_vec()));
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(
        url,
        InstallKind::WindowsInstaller {
            dir: dir.path().to_path_buf(),
        },
    );
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let err =
        kantor_update::stage(&cfg, &available, &http, &ReadsVersion, &mut |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::Checksum(_)), "{err:?}");
    assert!(
        leftovers(dir.path()).is_empty(),
        "{:?}",
        leftovers(dir.path())
    );
    // A manifest without a Windows build: offered, not installable.
    let url = publish(&base, &routes, "0.2.2", "linux-x86_64-appimage", b"x");
    let mut cfg = cfg;
    cfg.manifest_url = url;
    cfg.current = BuildId::new("0.2.0", 0).unwrap();
    assert!(
        kantor_update::check(&cfg, &http)
            .unwrap()
            .unwrap()
            .asset
            .is_none()
    );
}
