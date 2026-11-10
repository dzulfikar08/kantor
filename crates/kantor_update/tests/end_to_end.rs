//! Check, stage and apply against a local HTTP server, for fake AppImage,
//! install.sh and macOS-bundle installs in temp dirs.

#![cfg(unix)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use kantor_update::http::Http;
use kantor_update::verify::{SystemVerifier, Verifier};
use kantor_update::{BuildId, Channel, Config, Error, InstallKind};
use ring::signature::{Ed25519KeyPair, KeyPair as _};

type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;

/// A tiny HTTP/1.1 server: one response per connection, from `routes`.
/// A status of 302 sends the body as the Location.
fn serve() -> (String, Routes) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let routes: Routes = Arc::default();
    let r = routes.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap() <= 2 {
                    break;
                }
            }
            let resp = r.lock().unwrap().get(&path).cloned();
            let _ = match resp {
                Some((302, to)) => write!(
                    stream,
                    "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    String::from_utf8(to).unwrap()
                ),
                Some((status, body)) => write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .and_then(|_| stream.write_all(&body)),
                None => write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            };
        }
    });
    (base, routes)
}

fn key() -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&[42; 32]).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Publish a signed manifest for `version` with one asset under `asset_key`.
fn publish(base: &str, routes: &Routes, version: &str, asset_key: &str, asset: &[u8]) -> String {
    let manifest = format!(
        r#"{{
  "channel": "stable",
  "version": "{version}",
  "date": "2026-11-10T00:00:00Z",
  "notes_url": "{base}/notes",
  "assets": {{
    "{asset_key}": {{ "url": "{base}/redirect-asset", "sha256": "{}", "size": {} }}
  }}
}}
"#,
        sha256_hex(asset),
        asset.len()
    );
    let sig = base64::engine::general_purpose::STANDARD.encode(key().sign(manifest.as_bytes()));
    let mut r = routes.lock().unwrap();
    // The site redirects the manifest, GitHub redirects the asset.
    r.insert(
        "/update/stable.json".into(),
        (302, format!("{base}/gh/update-stable.json").into_bytes()),
    );
    r.insert(
        "/gh/update-stable.json".into(),
        (200, manifest.into_bytes()),
    );
    r.insert("/update/stable.json.sig".into(), (200, sig.into_bytes()));
    r.insert(
        "/redirect-asset".into(),
        (302, format!("{base}/asset").into_bytes()),
    );
    r.insert("/asset".into(), (200, asset.to_vec()));
    format!("{base}/update/stable.json")
}

fn config(url: String, kind: InstallKind) -> Config {
    Config {
        channel: Channel::Stable,
        manifest_url: url,
        public_key: key().public_key().as_ref().try_into().unwrap(),
        current: BuildId::new("0.2.0", 0).unwrap(),
        kind,
        arch: "x86_64".into(),
    }
}

/// A shell script standing in for Kantor: `--version` prints `kantor VERSION`.
fn fake_kantor(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho \"kantor {version}\"\n").into_bytes()
}

fn write_exec(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_version(path: &Path) -> String {
    SystemVerifier
        .probe(path, &["--version"])
        .unwrap()
        .trim()
        .to_owned()
}

fn leftovers(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    v.sort();
    v
}

#[test]
fn appimage_update() {
    let (base, routes) = serve();
    let url = publish(
        &base,
        &routes,
        "0.2.1",
        "linux-x86_64-appimage",
        &fake_kantor("0.2.1"),
    );
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("Kantor-x86_64.AppImage");
    write_exec(&image, &fake_kantor("0.2.0"));
    let cfg = config(
        url,
        InstallKind::AppImage {
            path: image.clone(),
        },
    );
    let http = Http::new();

    let available = kantor_update::check(&cfg, &http)
        .unwrap()
        .expect("0.2.1 > 0.2.0");
    assert_eq!(available.display_version(), "0.2.1");
    let mut seen = Vec::new();
    let staged = kantor_update::stage(&cfg, &available, &http, &SystemVerifier, &mut |d, t| {
        seen.push((d, t))
    })
    .unwrap();
    let size = fake_kantor("0.2.1").len() as u64;
    assert_eq!(
        seen.last(),
        Some(&(size, size)),
        "progress reaches the size"
    );
    assert_eq!(
        run_version(&image),
        "kantor 0.2.0",
        "nothing changes before apply"
    );
    assert!(kantor_update::staged_is_current(&staged, &cfg.current));

    kantor_update::apply(&staged).unwrap();
    assert_eq!(run_version(&image), "kantor 0.2.1");
    let mode = std::fs::metadata(&image).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    assert!(
        leftovers(dir.path()).is_empty(),
        "{:?}",
        leftovers(dir.path())
    );
    assert!(!kantor_update::staged_is_current(&staged, &cfg.current));

    // The same manifest again: nothing newer than 0.2.1.
    let mut cfg = cfg;
    cfg.current = BuildId::new("0.2.1", 0).unwrap();
    assert!(kantor_update::check(&cfg, &http).unwrap().is_none());
}

#[test]
fn corrupt_download_leaves_install_alone() {
    let (base, routes) = serve();
    let url = publish(
        &base,
        &routes,
        "0.2.1",
        "linux-x86_64-appimage",
        &fake_kantor("0.2.1"),
    );
    // Same length, different bytes.
    routes
        .lock()
        .unwrap()
        .insert("/asset".into(), (200, fake_kantor("6.6.6")));
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("Kantor-x86_64.AppImage");
    write_exec(&image, &fake_kantor("0.2.0"));
    let cfg = config(
        url,
        InstallKind::AppImage {
            path: image.clone(),
        },
    );
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let err =
        kantor_update::stage(&cfg, &available, &http, &SystemVerifier, &mut |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::Checksum(_)), "{err:?}");
    assert_eq!(run_version(&image), "kantor 0.2.0");
    assert!(leftovers(dir.path()).is_empty());

    // Longer than promised: stopped while downloading.
    routes
        .lock()
        .unwrap()
        .insert("/asset".into(), (200, vec![b'x'; 1000]));
    let err =
        kantor_update::stage(&cfg, &available, &http, &SystemVerifier, &mut |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::Checksum(_)), "{err:?}");
    assert!(leftovers(dir.path()).is_empty());
}

#[test]
fn wrong_version_inside_is_refused() {
    let (base, routes) = serve();
    // Signed as 0.2.1 but the binary says 0.1.0 (a mislabelled build).
    let url = publish(
        &base,
        &routes,
        "0.2.1",
        "linux-x86_64-appimage",
        &fake_kantor("0.1.0"),
    );
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("Kantor.AppImage");
    write_exec(&image, &fake_kantor("0.2.0"));
    let cfg = config(
        url,
        InstallKind::AppImage {
            path: image.clone(),
        },
    );
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let err =
        kantor_update::stage(&cfg, &available, &http, &SystemVerifier, &mut |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::Verify(_)), "{err:?}");
    assert!(leftovers(dir.path()).is_empty());
}

#[test]
fn tampered_manifest_is_refused() {
    let (base, routes) = serve();
    let url = publish(&base, &routes, "0.2.1", "linux-x86_64-appimage", b"x");
    {
        let mut r = routes.lock().unwrap();
        let (_, m) = r["/gh/update-stable.json"].clone();
        let m = String::from_utf8(m).unwrap().replace("0.2.1", "0.2.2");
        r.insert("/gh/update-stable.json".into(), (200, m.into_bytes()));
    }
    let cfg = config(url, InstallKind::Unmanaged);
    assert_eq!(
        kantor_update::check(&cfg, &Http::new()).unwrap_err(),
        Error::Signature
    );
}

/// Build `name.tar.gz` from `dir/name` with the system tar.
fn tar_gz(dir: &Path, name: &str) -> Vec<u8> {
    let out = dir.join("out.tar.gz");
    let st = Command::new("tar")
        .arg("-C")
        .arg(dir)
        .arg("-czf")
        .arg(&out)
        .arg(name)
        .status()
        .unwrap();
    assert!(st.success());
    std::fs::read(out).unwrap()
}

#[test]
fn tarball_update() {
    let (base, routes) = serve();
    // What scripts/package.sh builds.
    let src = tempfile::tempdir().unwrap();
    let pkg = src.path().join("kantor-0.2.1-x86_64-linux");
    write_exec(&pkg.join("bin/kantor"), &fake_kantor("0.2.1"));
    std::fs::create_dir_all(pkg.join("share/applications")).unwrap();
    std::fs::write(
        pkg.join("share/applications/kantor.desktop"),
        "[Desktop Entry]\nName=Kantor\nExec=kantor %F\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("share/applications/kantor-sheets.desktop"),
        "[Desktop Entry]\nName=Kantor Sheets\nExec=kantor --sheets %F\n",
    )
    .unwrap();
    std::fs::create_dir_all(pkg.join("share/icons/hicolor/scalable/apps")).unwrap();
    std::fs::write(
        pkg.join("share/icons/hicolor/scalable/apps/kantor.svg"),
        "<svg new/>",
    )
    .unwrap();
    let archive = tar_gz(src.path(), "kantor-0.2.1-x86_64-linux");
    let url = publish(&base, &routes, "0.2.1", "linux-x86_64-tarball", &archive);

    // What install.sh left in ~/.local.
    let home = tempfile::tempdir().unwrap();
    let prefix = home.path().join(".local");
    let bin = prefix.join("bin/kantor");
    write_exec(&bin, &fake_kantor("0.2.0"));
    let apps = prefix.join("share/applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(
        apps.join("kantor.desktop"),
        format!("[Desktop Entry]\nName=Kantor\nExec={} %F\n", bin.display()),
    )
    .unwrap();
    std::fs::write(apps.join("other-app.desktop"), "keep me").unwrap();
    let kind = kantor_update::platform::detect_from(&bin, None, None, "linux");
    assert_eq!(
        kind,
        InstallKind::Tarball {
            prefix: prefix.clone()
        }
    );

    let cfg = config(url, kind);
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let staged =
        kantor_update::stage(&cfg, &available, &http, &SystemVerifier, &mut |_, _| {}).unwrap();
    kantor_update::apply(&staged).unwrap();

    assert_eq!(run_version(&bin), "kantor 0.2.1");
    let desktop = std::fs::read_to_string(apps.join("kantor.desktop")).unwrap();
    assert!(
        desktop.contains(&format!("Exec={} %F", bin.display())),
        "{desktop}"
    );
    let sheets = std::fs::read_to_string(apps.join("kantor-sheets.desktop")).unwrap();
    assert!(
        sheets.contains(&format!("Exec={} --sheets %F", bin.display())),
        "{sheets}"
    );
    assert_eq!(
        std::fs::read_to_string(apps.join("other-app.desktop")).unwrap(),
        "keep me"
    );
    assert_eq!(
        std::fs::read_to_string(prefix.join("share/icons/hicolor/scalable/apps/kantor.svg"))
            .unwrap(),
        "<svg new/>"
    );
    assert!(leftovers(&prefix).is_empty(), "{:?}", leftovers(&prefix));
    assert!(leftovers(&prefix.join("bin")).is_empty());
    // Still recognised as an install.sh install after the update.
    assert_eq!(
        kantor_update::platform::detect_from(&bin, None, None, "linux"),
        InstallKind::Tarball { prefix }
    );
}

/// Stands in for codesign/spctl: records the app it was asked about.
struct FakeMac {
    accept: bool,
    checked: Mutex<Vec<PathBuf>>,
}

impl Verifier for FakeMac {
    fn check_app(&self, app: &Path) -> Result<(), Error> {
        self.checked.lock().unwrap().push(app.to_path_buf());
        if self.accept {
            Ok(())
        } else {
            Err(Error::Verify("signed by team XXXXXXXXXX".into()))
        }
    }
    fn probe(&self, program: &Path, args: &[&str]) -> Result<String, Error> {
        SystemVerifier.probe(program, args)
    }
}

#[test]
fn mac_bundle_update() {
    let (base, routes) = serve();
    let src = tempfile::tempdir().unwrap();
    let pkg = src.path().join("kantor-0.2.1-universal-macos");
    write_exec(
        &pkg.join("Kantor.app/Contents/MacOS/kantor"),
        &fake_kantor("0.2.1"),
    );
    std::fs::write(pkg.join("Kantor.app/Contents/Info.plist"), "new").unwrap();
    std::fs::write(pkg.join("README.md"), "readme").unwrap();
    let archive = tar_gz(src.path(), "kantor-0.2.1-universal-macos");
    let url = publish(&base, &routes, "0.2.1", "macos-universal-app", &archive);

    let apps = tempfile::tempdir().unwrap();
    let bundle = apps.path().join("Kantor.app");
    write_exec(&bundle.join("Contents/MacOS/kantor"), &fake_kantor("0.2.0"));
    std::fs::write(bundle.join("Contents/Info.plist"), "old").unwrap();
    std::fs::write(bundle.join("Contents/stale-file"), "old").unwrap();
    let exe = bundle.join("Contents/MacOS/kantor");
    let kind = kantor_update::platform::detect_from(&exe, None, None, "macos");
    let cfg = config(url, kind);
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();

    // Gatekeeper says no: nothing staged, nothing left behind.
    let reject = FakeMac {
        accept: false,
        checked: Mutex::default(),
    };
    let err = kantor_update::stage(&cfg, &available, &http, &reject, &mut |_, _| {}).unwrap_err();
    assert!(matches!(err, Error::Verify(_)));
    assert!(
        leftovers(apps.path()).is_empty(),
        "{:?}",
        leftovers(apps.path())
    );
    assert_eq!(run_version(&exe), "kantor 0.2.0");

    let accept = FakeMac {
        accept: true,
        checked: Mutex::default(),
    };
    let staged = kantor_update::stage(&cfg, &available, &http, &accept, &mut |_, _| {}).unwrap();
    let checked = accept.checked.lock().unwrap().clone();
    assert_eq!(checked.len(), 1);
    assert!(checked[0].ends_with("Kantor.app"));
    kantor_update::apply(&staged).unwrap();
    assert_eq!(run_version(&exe), "kantor 0.2.1");
    assert_eq!(
        std::fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap(),
        "new"
    );
    assert!(!bundle.join("Contents/stale-file").exists());
    assert!(
        leftovers(apps.path()).is_empty(),
        "{:?}",
        leftovers(apps.path())
    );
}

#[test]
fn read_only_install_says_not_writable() {
    let (base, routes) = serve();
    let url = publish(
        &base,
        &routes,
        "0.2.1",
        "linux-x86_64-appimage",
        &fake_kantor("0.2.1"),
    );
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("opt");
    let image = ro.join("Kantor.AppImage");
    write_exec(&image, &fake_kantor("0.2.0"));
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if tempfile::NamedTempFile::new_in(&ro).is_ok() {
        // Running as root: permissions don't apply.
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let cfg = config(
        url,
        InstallKind::AppImage {
            path: image.clone(),
        },
    );
    let http = Http::new();
    let available = kantor_update::check(&cfg, &http).unwrap().unwrap();
    let err =
        kantor_update::stage(&cfg, &available, &http, &SystemVerifier, &mut |_, _| {}).unwrap_err();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(err, Error::NotWritable(_)), "{err:?}");
    assert_eq!(run_version(&image), "kantor 0.2.0");
}

/// scripts/update-manifest.sh (openssl) signs what the app (ring) verifies,
/// and embeds the same public key.
#[test]
fn ci_signer_matches_the_app() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/update-manifest.sh");
    let text = std::fs::read_to_string(&script).unwrap();
    if option_env!("KANTOR_UPDATE_PUBKEY").is_none() {
        assert!(
            text.contains(&format!(
                "EMBEDDED_PUBLIC_KEY=\"{}\"",
                kantor_update::manifest::PUBLIC_KEY
            )),
            "the script's key differs from manifest.rs"
        );
    }
    let openssl3 = Command::new("openssl")
        .arg("version")
        .output()
        .is_ok_and(|o| o.stdout.starts_with(b"OpenSSL 3"));
    if !openssl3 {
        eprintln!("skipped: needs OpenSSL 3 (macOS ships LibreSSL)");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("Kantor-0.2.1-x86_64.AppImage");
    std::fs::write(&asset, b"an appimage").unwrap();
    let out = dir.path().join("update-stable.json");
    let seed = base64::engine::general_purpose::STANDARD.encode([42u8; 32]);
    let public = base64::engine::general_purpose::STANDARD.encode(key().public_key().as_ref());
    let st = Command::new("bash")
        .arg(&script)
        .args([
            "stable",
            "0.2.1",
            "0",
            "https://example.com/notes",
            "https://example.com/dl",
        ])
        .arg(&out)
        .arg(format!("linux-x86_64-appimage={}", asset.display()))
        .env("UPDATE_SIGNING_KEY", seed)
        .env("UPDATE_PUBLIC_KEY", &public)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let bytes = std::fs::read(&out).unwrap();
    let sig = std::fs::read(out.with_extension("json.sig")).unwrap();
    let m = kantor_update::manifest::verify(
        &bytes,
        &sig,
        &key().public_key().as_ref().try_into().unwrap(),
    )
    .unwrap();
    let a = &m.assets["linux-x86_64-appimage"];
    assert_eq!(a.url, "https://example.com/dl/Kantor-0.2.1-x86_64.AppImage");
    assert_eq!(a.size, 11);
    assert_eq!(a.sha256, sha256_hex(b"an appimage"));
    // Wrong expected key: refused before anything is signed.
    let st = Command::new("bash")
        .arg(&script)
        .args(["stable", "0.2.1", "0", "https://e/n", "https://e/d"])
        .arg(dir.path().join("x.json"))
        .arg(format!("linux-x86_64-appimage={}", asset.display()))
        .env(
            "UPDATE_SIGNING_KEY",
            base64::engine::general_purpose::STANDARD.encode([1u8; 32]),
        )
        .env("UPDATE_PUBLIC_KEY", &public)
        .output()
        .unwrap();
    assert!(!st.status.success());
    assert!(!dir.path().join("x.json.sig").exists());
}
