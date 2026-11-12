//! Staging a downloaded build next to the install, and swapping it in.
//!
//! Staging does everything slow or fallible: download, size and SHA-256
//! check, unpack, code-signature checks (macOS), and a `--version` run of
//! the new binary. The swap itself is renames on one filesystem, done as a
//! transaction: every replaced file or bundle is kept until all swaps have
//! succeeded, and put back if any fails.
//!
//! Windows installs made by Kantor-Setup-*.exe don't swap files: the staged
//! update is the new installer (its SHA-256 checked against the signed
//! manifest), run silently once Kantor has quit (it waits for this process to
//! exit), optionally starting Kantor again. A running .exe can't be replaced on
//! Windows but can be renamed, so file swaps there move the old file aside
//! first; the leftover is removed on the next start ([`clean_leftovers`]).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::http::Http;
use crate::manifest::{Asset, Manifest};
use crate::platform::InstallKind;
use crate::verify::Verifier;

/// A verified build waiting to be swapped in (saved across runs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub version: String,
    #[serde(default)]
    pub build: u64,
    pub notes_url: String,
    pub kind: InstallKind,
    /// `(new, target)`: each `new` replaces its `target`, in order.
    pub swaps: Vec<(PathBuf, PathBuf)>,
    /// Scratch space removed after the swap (or when the update is dropped).
    pub work_dir: Option<PathBuf>,
    /// Windows installer installs: the downloaded Kantor-Setup-*.exe, run
    /// silently instead of swapping files.
    #[serde(default)]
    pub installer: Option<PathBuf>,
}

/// Download `asset` to `dest`, checking its size and SHA-256. `dest` is
/// removed on any failure.
pub fn download_verified(
    http: &Http,
    asset: &Asset,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), Error> {
    let result = (|| {
        let file = create(dest)?;
        let mut out = HashingWriter {
            inner: io::BufWriter::new(file),
            digest: ring::digest::Context::new(&ring::digest::SHA256),
        };
        let total = asset.size;
        let n = http.download(&asset.url, &mut out, total, &mut |done| {
            progress(done, total)
        })?;
        out.inner.flush()?;
        out.inner.get_ref().sync_all()?;
        check_digest(n, out.digest.finish().as_ref(), asset)
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

/// The downloaded byte count and digest match the manifest.
pub fn check_digest(size: u64, digest: &[u8], asset: &Asset) -> Result<(), Error> {
    if size != asset.size {
        return Err(Error::Checksum(format!(
            "got {size} bytes, the manifest says {}",
            asset.size
        )));
    }
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != asset.sha256 {
        return Err(Error::Checksum(
            "SHA-256 doesn't match the signed manifest".into(),
        ));
    }
    Ok(())
}

struct HashingWriter<W> {
    inner: W,
    digest: ring::digest::Context,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.digest.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn not_writable(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    )
}

fn create(path: &Path) -> Result<fs::File, Error> {
    fs::File::create(path).map_err(|e| {
        if not_writable(&e) {
            Error::NotWritable(path.parent().unwrap_or(path).to_path_buf())
        } else {
            Error::Io(format!("{}: {e}", path.display()))
        }
    })
}

fn create_dir(path: &Path) -> Result<(), Error> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| io_err(path, e))?;
    }
    fs::create_dir_all(path).map_err(|e| {
        if not_writable(&e) {
            Error::NotWritable(path.parent().unwrap_or(path).to_path_buf())
        } else {
            io_err(path, e)
        }
    })
}

/// Renames into `dir` will work: Kantor can create files there.
fn check_writable(dir: &Path) -> Result<(), Error> {
    fs::create_dir_all(dir)
        .and_then(|_| tempfile::NamedTempFile::new_in(dir))
        .map(drop)
        .map_err(|e| {
            if not_writable(&e) {
                Error::NotWritable(dir.to_path_buf())
            } else {
                io_err(dir, e)
            }
        })
}

fn io_err(path: &Path, e: io::Error) -> Error {
    Error::Io(format!("{}: {e}", path.display()))
}

/// The new build starts and says it is `version`.
fn probe_version(
    verifier: &dyn Verifier,
    program: &Path,
    args: &[&str],
    version: &str,
) -> Result<(), Error> {
    let out = verifier.probe(program, args)?;
    let reported = out.lines().next().unwrap_or_default().trim();
    // `kantor 0.2.1` or `kantor 0.2.1 (nightly …)`.
    if reported.split_whitespace().nth(1) != Some(version) {
        return Err(Error::Verify(format!(
            "the new build reports {reported:?}, expected kantor {version}"
        )));
    }
    Ok(())
}

/// Download and check `asset` for the install `kind`, leaving it ready to
/// swap in (see [`apply`]).
pub fn stage(
    manifest: &Manifest,
    asset: &Asset,
    kind: &InstallKind,
    http: &Http,
    verifier: &dyn Verifier,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Staged, Error> {
    let staged = |swaps, work_dir| Staged {
        version: manifest.version.clone(),
        build: manifest.build,
        notes_url: manifest.notes_url.clone(),
        kind: kind.clone(),
        swaps,
        work_dir,
        installer: None,
    };
    match kind {
        InstallKind::Unmanaged => Err(Error::Unmanaged),
        InstallKind::AppImage { path } => {
            let dir = path.parent().ok_or(Error::Unmanaged)?;
            let name = path.file_name().ok_or(Error::Unmanaged)?.to_string_lossy();
            // Next to the AppImage, so the swap is one rename on one filesystem.
            let new = dir.join(format!(".{name}.update"));
            download_verified(http, asset, &new, progress)?;
            let result = (|| {
                set_executable(&new)?;
                // Through FUSE as usual; without FUSE the runtime fails, so
                // unpack-and-run instead (slower, writes to $TMPDIR).
                probe_version(verifier, &new, &["--version"], &manifest.version).or_else(|_| {
                    probe_version(
                        verifier,
                        &new,
                        &["--appimage-extract-and-run", "--version"],
                        &manifest.version,
                    )
                })
            })();
            if let Err(e) = result {
                let _ = fs::remove_file(&new);
                return Err(e);
            }
            Ok(staged(vec![(new, path.clone())], None))
        }
        InstallKind::Tarball { prefix } => {
            let work = prefix.join(".kantor-update");
            let result = (|| {
                create_dir(&work)?;
                let archive = work.join("download.tar.gz");
                download_verified(http, asset, &archive, progress)?;
                let root = unpack(&archive, &work.join("x"), |d| {
                    d.join("bin/kantor").is_file()
                })?;
                let _ = fs::remove_file(&archive);
                let bin = root.join("bin/kantor");
                set_executable(&bin)?;
                probe_version(verifier, &bin, &["--version"], &manifest.version)?;

                let mut swaps = vec![(bin, prefix.join("bin/kantor"))];
                // Launchers point at this prefix's binary, as install.sh writes them.
                for new in files_in(&root.join("share/applications"), "desktop") {
                    let text = fs::read_to_string(&new).map_err(|e| io_err(&new, e))?;
                    let exec = format!("Exec={}/bin/kantor", prefix.display());
                    let text: String = text
                        .lines()
                        .map(|l| match l.strip_prefix("Exec=kantor") {
                            Some(rest) => format!("{exec}{rest}\n"),
                            None => format!("{l}\n"),
                        })
                        .collect();
                    fs::write(&new, text).map_err(|e| io_err(&new, e))?;
                    let target = prefix
                        .join("share/applications")
                        .join(new.file_name().unwrap_or_default());
                    swaps.push((new, target));
                }
                let icons = "share/icons/hicolor/scalable/apps";
                for new in files_in(&root.join(icons), "svg") {
                    let target = prefix.join(icons).join(new.file_name().unwrap_or_default());
                    swaps.push((new, target));
                }
                for (_, target) in &swaps {
                    if let Some(dir) = target.parent() {
                        check_writable(dir)?;
                    }
                }
                Ok(swaps)
            })();
            match result {
                Ok(swaps) => Ok(staged(swaps, Some(work))),
                Err(e) => {
                    let _ = fs::remove_dir_all(&work);
                    Err(e)
                }
            }
        }
        InstallKind::WindowsInstaller { dir } => {
            let work = dir.join(WORK_DIR);
            let result = (|| {
                check_writable(dir)?;
                create_dir(&work)?;
                let setup = work.join("Kantor-Setup.exe");
                download_verified(http, asset, &setup, progress)?;
                Ok(setup)
            })();
            match result {
                Ok(setup) => Ok(Staged {
                    installer: Some(setup),
                    ..staged(Vec::new(), Some(work))
                }),
                Err(e) => {
                    let _ = fs::remove_dir_all(&work);
                    Err(e)
                }
            }
        }
        InstallKind::WindowsPortable { dir } => {
            let work = dir.join(WORK_DIR);
            let result = (|| {
                create_dir(&work)?;
                let archive = work.join("download.zip");
                download_verified(http, asset, &archive, progress)?;
                let root = unzip(&archive, &work.join("x"), |d| {
                    d.join("kantor.exe").is_file()
                })?;
                let _ = fs::remove_file(&archive);
                probe_version(
                    verifier,
                    &root.join("kantor.exe"),
                    &["--version"],
                    &manifest.version,
                )?;
                // Every file at the top of the zip (kantor.exe, kantor.com,
                // the marker, licences); the zip has no subfolders.
                let swaps: Vec<(PathBuf, PathBuf)> = fs::read_dir(&root)
                    .map_err(|e| io_err(&root, e))?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.is_file())
                    .map(|p| {
                        let target = dir.join(p.file_name().unwrap_or_default());
                        (p, target)
                    })
                    .collect();
                check_writable(dir)?;
                Ok(swaps)
            })();
            match result {
                Ok(mut swaps) => {
                    // kantor.exe last: everything else is in place by the
                    // time the new build could start.
                    swaps.sort_by_key(|(_, t)| {
                        t.file_name()
                            .is_some_and(|n| n.eq_ignore_ascii_case("kantor.exe"))
                    });
                    Ok(staged(swaps, Some(work)))
                }
                Err(e) => {
                    let _ = fs::remove_dir_all(&work);
                    Err(e)
                }
            }
        }
        InstallKind::MacApp { bundle } => {
            let parent = bundle.parent().ok_or(Error::Unmanaged)?;
            let work = parent.join(".Kantor-update");
            let result = (|| {
                create_dir(&work)?;
                check_writable(parent)?;
                let archive = work.join("download.tar.gz");
                download_verified(http, asset, &archive, progress)?;
                let root = unpack(&archive, &work.join("x"), |d| {
                    d.join("Kantor.app/Contents/MacOS/kantor").is_file()
                })?;
                let _ = fs::remove_file(&archive);
                let app = root.join("Kantor.app");
                verifier.check_app(&app)?;
                probe_version(
                    verifier,
                    &app.join("Contents/MacOS/kantor"),
                    &["--version"],
                    &manifest.version,
                )?;
                Ok(app)
            })();
            match result {
                Ok(app) => Ok(staged(vec![(app, bundle.clone())], Some(work))),
                Err(e) => {
                    let _ = fs::remove_dir_all(&work);
                    Err(e)
                }
            }
        }
    }
}

/// Scratch folder for Windows updates, inside the install folder.
const WORK_DIR: &str = ".kantor-update";

/// Unpack a .zip into `into` (no tools needed; entries that would land outside
/// `into` are refused) and return the folder `is_root` accepts: `into` itself
/// or a top-level folder in it.
fn unzip(archive: &Path, into: &Path, is_root: impl Fn(&Path) -> bool) -> Result<PathBuf, Error> {
    let file = fs::File::open(archive).map_err(|e| io_err(archive, e))?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(file))
        .map_err(|e| Error::Io(format!("the update archive is not a zip: {e}")))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| Error::Io(format!("reading the update archive: {e}")))?;
        let rel = entry.enclosed_name().ok_or_else(|| {
            Error::Io(format!("unsafe path in the update: {:?}", entry.name_raw()))
        })?;
        let out = into.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| io_err(&out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        let mut f = create(&out)?;
        io::copy(&mut entry, &mut f).map_err(|e| io_err(&out, e))?;
        f.sync_all().map_err(|e| io_err(&out, e))?;
    }
    if is_root(into) {
        return Ok(into.to_path_buf());
    }
    fs::read_dir(into)
        .map_err(|e| io_err(into, e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_dir() && is_root(p))
        .ok_or_else(|| Error::Io("the update archive doesn't hold Kantor".into()))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| io_err(path, e))
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), Error> {
    Ok(())
}

/// Unpack a .tar.gz with the system `tar` (GNU tar on Linux, bsdtar on
/// macOS, which keeps the app's extended attributes) and return the
/// directory inside it that `is_root` accepts (the archive's top folder).
fn unpack(archive: &Path, into: &Path, is_root: impl Fn(&Path) -> bool) -> Result<PathBuf, Error> {
    fs::create_dir_all(into).map_err(|e| io_err(into, e))?;
    let out = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|e| Error::Io(format!("can't run tar: {e}")))?;
    if !out.status.success() {
        return Err(Error::Io(format!(
            "unpacking the update failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    if is_root(into) {
        return Ok(into.to_path_buf());
    }
    fs::read_dir(into)
        .map_err(|e| io_err(into, e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_dir() && is_root(p))
        .ok_or_else(|| Error::Io("the update archive doesn't hold Kantor".into()))
}

fn files_in(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension() == Some(ext.as_ref()))
        .collect();
    v.sort();
    v
}

// ── swapping ─────────────────────────────────────────────────────────────

/// One swap done, with what it replaced.
struct Done {
    target: PathBuf,
    backup: Option<PathBuf>,
}

fn backup_path(target: &Path) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!(".{name}.kantor-old"))
}

fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn swap_one(new: &Path, target: &Path) -> io::Result<Done> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let backup = backup_path(target);
    remove_any(&backup)?;
    match fs::symlink_metadata(target) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::rename(new, target)?;
            Ok(Done {
                target: target.to_path_buf(),
                backup: None,
            })
        }
        Err(e) => Err(e),
        Ok(m) if m.is_dir() => {
            // A bundle: move it aside, move the new one in.
            fs::rename(target, &backup)?;
            if let Err(e) = fs::rename(new, target) {
                fs::rename(&backup, target)?;
                return Err(e);
            }
            Ok(Done {
                target: target.to_path_buf(),
                backup: Some(backup),
            })
        }
        Ok(_) if cfg!(windows) => {
            // A running .exe can be renamed but not replaced or deleted:
            // move the old file aside, then the new one in.
            fs::rename(target, &backup)?;
            if let Err(e) = fs::rename(new, target) {
                fs::rename(&backup, target)?;
                return Err(e);
            }
            Ok(Done {
                target: target.to_path_buf(),
                backup: Some(backup),
            })
        }
        Ok(_) => {
            // A file: keep a link to the old one, then replace it in one
            // rename, so the path never goes missing.
            if fs::hard_link(target, &backup).is_err() {
                fs::copy(target, &backup)?;
            }
            if let Err(e) = fs::rename(new, target) {
                let _ = fs::remove_file(&backup);
                return Err(e);
            }
            Ok(Done {
                target: target.to_path_buf(),
                backup: Some(backup),
            })
        }
    }
}

fn roll_back(done: Vec<Done>) {
    for d in done.into_iter().rev() {
        let restored = match &d.backup {
            Some(b) => remove_any(&d.target).and_then(|_| fs::rename(b, &d.target)),
            None => remove_any(&d.target),
        };
        if let Err(e) = restored {
            log::error!("update rollback: {}: {e}", d.target.display());
        }
    }
}

/// Swap every `(new, target)` in, or none of them.
pub fn swap_all(swaps: &[(PathBuf, PathBuf)]) -> Result<(), Error> {
    if let Some((new, _)) = swaps
        .iter()
        .find(|(new, _)| fs::symlink_metadata(new).is_err())
    {
        return Err(Error::Io(format!(
            "the downloaded update is gone ({})",
            new.display()
        )));
    }
    let mut done = Vec::new();
    for (new, target) in swaps {
        match swap_one(new, target) {
            Ok(d) => done.push(d),
            Err(e) => {
                roll_back(done);
                return Err(if not_writable(&e) {
                    Error::NotWritable(target.parent().unwrap_or(target).to_path_buf())
                } else {
                    io_err(target, e)
                });
            }
        }
    }
    for d in done {
        if let Some(b) = d.backup
            && let Err(e) = remove_any(&b)
        {
            // Windows: the running kantor.exe; gone on the next start.
            log::info!("update: couldn't remove {} yet: {e}", b.display());
        }
    }
    Ok(())
}

/// At start-up: remove what an applied update left in the install folder
/// (Windows: the old kantor.exe, which was still running, and the installer
/// that ran). `staged` is the update still waiting, whose files stay.
pub fn clean_leftovers(kind: &InstallKind, staged: Option<&Staged>) {
    let dir = match kind {
        InstallKind::WindowsInstaller { dir } | InstallKind::WindowsPortable { dir } => dir,
        _ => return,
    };
    let work = dir.join(WORK_DIR);
    let keep = staged.is_some_and(|s| s.work_dir.as_deref() == Some(work.as_path()));
    if !keep && work.exists() {
        let _ = fs::remove_dir_all(&work);
    }
    for path in fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
    {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') && name.ends_with(".kantor-old") {
            let _ = remove_any(&path);
        }
    }
}

/// The silent run of a downloaded Kantor-Setup-*.exe (Inno Setup; see
/// packaging/windows/kantor.iss): no wizard, no prompts, waits for process
/// `pid` (this Kantor) to exit before touching files, and with `relaunch`
/// starts Kantor when done.
pub fn installer_command(setup: &Path, pid: u32, relaunch: bool) -> Command {
    let mut c = Command::new(setup);
    c.args([
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/NOCANCEL",
        "/SP-",
    ])
    .arg(format!("/WAITPID={pid}"));
    if relaunch {
        c.arg("/RELAUNCH=1");
    }
    c
}

fn run_installer(setup: &Path, relaunch: bool) -> Result<(), Error> {
    let mut cmd = installer_command(setup, std::process::id(), relaunch);
    detach(&mut cmd);
    cmd.spawn()
        .map(drop)
        .map_err(|e| Error::Io(format!("can't start the installer: {e}")))
}

/// Started on its own: no inherited stdio, not in this process's group (and
/// on Windows no console), so it outlives Kantor quitting.
fn detach(cmd: &mut Command) {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
}

/// Swap the staged build in, then drop the scratch space. A Windows
/// installer update starts the installer, which finishes once Kantor exits.
pub fn apply(staged: &Staged) -> Result<(), Error> {
    if let Some(setup) = &staged.installer {
        // Its folder goes on the next start: the installer runs from it.
        return run_installer(setup, false);
    }
    swap_all(&staged.swaps)?;
    discard(staged);
    Ok(())
}

/// [`apply`], then start the new build ("Restart to update").
pub fn apply_and_relaunch(staged: &Staged) -> Result<(), Error> {
    if let Some(setup) = &staged.installer {
        return run_installer(setup, true);
    }
    apply(staged)?;
    if let Err(e) = relaunch(&staged.kind) {
        log::error!("update: installed, but relaunching failed: {e}");
    }
    Ok(())
}

/// Remove whatever is left of a staged update.
pub fn discard(staged: &Staged) {
    for new in staged.swaps.iter().map(|(n, _)| n).chain(&staged.installer) {
        let _ = remove_any(new);
    }
    if let Some(work) = &staged.work_dir {
        let _ = fs::remove_dir_all(work);
    }
}

/// Start the installed (just updated) Kantor, detached from this process.
pub fn relaunch(kind: &InstallKind) -> io::Result<()> {
    let mut cmd = match kind {
        InstallKind::AppImage { path } => {
            let mut c = Command::new(path);
            // The new AppImage runtime sets its own.
            for k in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
                c.env_remove(k);
            }
            c
        }
        InstallKind::Tarball { prefix } => Command::new(prefix.join("bin/kantor")),
        InstallKind::MacApp { bundle } => {
            let mut c = Command::new("/usr/bin/open");
            c.arg("-n").arg(bundle);
            c
        }
        InstallKind::WindowsInstaller { dir } | InstallKind::WindowsPortable { dir } => {
            Command::new(dir.join("kantor.exe"))
        }
        InstallKind::Unmanaged => {
            return Err(io::Error::other("not an updatable install"));
        }
    };
    detach(&mut cmd);
    cmd.spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn silent_installer_arguments() {
        let c = installer_command(Path::new(r"C:\K\.kantor-update\Kantor-Setup.exe"), 42, true);
        let args: Vec<_> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/NOCANCEL",
                "/SP-",
                "/WAITPID=42",
                "/RELAUNCH=1"
            ]
        );
        let c = installer_command(Path::new("setup.exe"), 7, false);
        assert!(!c.get_args().any(|a| a == "/RELAUNCH=1"));
    }

    #[test]
    fn unzip_refuses_paths_outside() {
        let d = tempfile::tempdir().unwrap();
        let archive = d.path().join("evil.zip");
        {
            let mut z = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("../escaped.txt", opts).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let into = d.path().join("x");
        assert!(unzip(&archive, &into, |_| true).is_err());
        assert!(!d.path().join("escaped.txt").exists());
    }

    #[test]
    fn leftovers_are_cleaned() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        write(&p.join("kantor.exe"), "new");
        write(&p.join(".kantor.exe.kantor-old"), "old");
        write(&p.join(".kantor-update/Kantor-Setup.exe"), "setup");
        write(&p.join("notes.txt"), "mine");
        let kind = InstallKind::WindowsInstaller {
            dir: p.to_path_buf(),
        };
        clean_leftovers(&kind, None);
        assert!(!p.join(".kantor.exe.kantor-old").exists());
        assert!(!p.join(".kantor-update").exists());
        assert_eq!(read(&p.join("notes.txt")), "mine");
        // A staged update's folder stays.
        write(&p.join(".kantor-update/Kantor-Setup.exe"), "setup");
        let staged = Staged {
            version: "9.9.9".into(),
            build: 0,
            notes_url: String::new(),
            kind: kind.clone(),
            swaps: Vec::new(),
            work_dir: Some(p.join(".kantor-update")),
            installer: Some(p.join(".kantor-update/Kantor-Setup.exe")),
        };
        clean_leftovers(&kind, Some(&staged));
        assert!(p.join(".kantor-update/Kantor-Setup.exe").exists());
    }

    #[test]
    fn digest_mismatch() {
        let digest = ring::digest::digest(&ring::digest::SHA256, b"hello");
        let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let asset = Asset {
            url: "https://x/y".into(),
            sha256: hex,
            size: 5,
        };
        check_digest(5, digest.as_ref(), &asset).unwrap();
        assert!(matches!(
            check_digest(6, digest.as_ref(), &asset),
            Err(Error::Checksum(_))
        ));
        let other = ring::digest::digest(&ring::digest::SHA256, b"hellO");
        assert!(matches!(
            check_digest(5, other.as_ref(), &asset),
            Err(Error::Checksum(_))
        ));
    }

    #[test]
    fn swaps_files_and_bundles() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        // An AppImage-like file, a tarball's launcher (new file), a bundle.
        write(&p.join("Kantor.AppImage"), "old image");
        write(&p.join(".Kantor.AppImage.update"), "new image");
        write(&p.join("stage/kantor-sheets.desktop"), "new launcher");
        write(&p.join("Kantor.app/Contents/MacOS/kantor"), "old app");
        write(&p.join("Kantor.app/Contents/Old-only"), "x");
        write(&p.join("stage/Kantor.app/Contents/MacOS/kantor"), "new app");
        let swaps = vec![
            (p.join(".Kantor.AppImage.update"), p.join("Kantor.AppImage")),
            (
                p.join("stage/kantor-sheets.desktop"),
                p.join("share/applications/kantor-sheets.desktop"),
            ),
            (p.join("stage/Kantor.app"), p.join("Kantor.app")),
        ];
        swap_all(&swaps).unwrap();
        assert_eq!(read(&p.join("Kantor.AppImage")), "new image");
        assert_eq!(
            read(&p.join("share/applications/kantor-sheets.desktop")),
            "new launcher"
        );
        assert_eq!(read(&p.join("Kantor.app/Contents/MacOS/kantor")), "new app");
        assert!(!p.join("Kantor.app/Contents/Old-only").exists());
        // No backups or staged files left behind.
        assert!(!p.join(".Kantor.AppImage.kantor-old").exists());
        assert!(!p.join(".Kantor.app.kantor-old").exists());
        assert!(!p.join(".Kantor.AppImage.update").exists());
    }

    #[test]
    fn failure_rolls_everything_back() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        write(&p.join("bin/kantor"), "old bin");
        write(&p.join("new/kantor"), "new bin");
        write(&p.join("Kantor.app/Contents/MacOS/kantor"), "old app");
        write(&p.join("new/Kantor.app/Contents/MacOS/kantor"), "new app");
        write(&p.join("new/kantor.desktop"), "new launcher");
        // The third target can't be created: its "directory" is a file.
        write(&p.join("blocked"), "a file, not a directory");
        let swaps = vec![
            (p.join("new/kantor"), p.join("bin/kantor")),
            (p.join("new/Kantor.app"), p.join("Kantor.app")),
            (
                p.join("new/kantor.desktop"),
                p.join("blocked/kantor.desktop"),
            ),
        ];
        assert!(swap_all(&swaps).is_err());
        assert_eq!(read(&p.join("bin/kantor")), "old bin");
        assert_eq!(read(&p.join("Kantor.app/Contents/MacOS/kantor")), "old app");
        assert!(!p.join("bin/.kantor.kantor-old").exists());
        assert!(!p.join(".Kantor.app.kantor-old").exists());
    }

    #[test]
    fn missing_staged_file_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        write(&p.join("bin/kantor"), "old");
        write(&p.join("new/kantor"), "new");
        let swaps = vec![
            (p.join("new/kantor"), p.join("bin/kantor")),
            (p.join("new/gone.desktop"), p.join("x.desktop")),
        ];
        assert!(swap_all(&swaps).is_err());
        assert_eq!(read(&p.join("bin/kantor")), "old");
        assert_eq!(read(&p.join("new/kantor")), "new");
    }

    #[cfg(unix)]
    #[test]
    fn read_only_dir_is_not_writable() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let ro = d.path().join("ro");
        fs::create_dir(&ro).unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores permissions; nothing to test then.
        if tempfile::NamedTempFile::new_in(&ro).is_err() {
            assert!(matches!(check_writable(&ro), Err(Error::NotWritable(_))));
            assert!(matches!(create(&ro.join("f")), Err(Error::NotWritable(_))));
        }
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
