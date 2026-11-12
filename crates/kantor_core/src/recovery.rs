//! Recovery copies of documents with unsaved changes, so a crash, a power cut
//! or a killed process doesn't lose work.
//!
//! The store lives in `<data dir>/recovery` (see [`crate::dirs::data_dir`]).
//! Each open document that has had unsaved changes gets a **slot**, a
//! directory holding:
//!
//! ```text
//! recovery/<slot id>/
//!   lock            held (OS file lock) by the Kantor process editing the document
//!   manifest.json   what the copy is: app, original file, time, Kantor version,
//!                   copy file name, length and checksum
//!   copy-<n>.xlsx   the document in its own format (docx / pptx for Docs / Slides)
//! ```
//!
//! * Writes are atomic: the copy is written to a temporary file, synced and
//!   renamed; then the manifest the same way (the manifest rename is the commit
//!   point); then the previous copy is deleted. A crash at any moment leaves a
//!   manifest that names a complete copy.
//! * The lock is an OS lock on an open file, so it is released when the
//!   process dies however it dies. A slot whose lock can be taken belongs to
//!   nobody: its document survived a crash. Slots of running Kantor processes
//!   (other windows too) are locked, so they are never offered or touched.
//! * A copy whose manifest can't be read, or whose length or checksum don't
//!   match, is damaged: [`Store::scan`] moves it to `recovery/damaged/` (never
//!   deletes it) and reports it.
//! * Writes are numbered by [`Slot::ticket`], taken when the document is
//!   snapshotted. A write older than the slot's current copy, or than its
//!   last [`Slot::mark_cleared`], is skipped, so a slow background write can't
//!   bring back a copy of a document that was saved meanwhile.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Manifest format; newer manifests are reported as damaged rather than
/// misread.
pub const FORMAT: u32 = 1;
const MANIFEST: &str = "manifest.json";
const LOCK: &str = "lock";
const DAMAGED: &str = "damaged";

/// The app a copy belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum App {
    Sheets,
    Docs,
    Slides,
}

impl App {
    /// The format copies are written in.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Sheets => "xlsx",
            Self::Docs => "docx",
            Self::Slides => "pptx",
        }
    }
}

/// Length and modification time of a file, to tell whether it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub len: u64,
    pub modified_secs: u64,
    pub modified_nanos: u32,
}

impl FileStamp {
    pub fn of(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            len: meta.len(),
            modified_secs: modified.as_secs(),
            modified_nanos: modified.subsec_nanos(),
        })
    }
}

/// The document a copy is of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocInfo {
    pub app: App,
    /// The name the tab showed (`budget.xlsx`, `Untitled 2.docx`).
    pub name: String,
    /// The file the document was opened from or last saved to; `None` for a
    /// document that was never saved.
    pub original: Option<PathBuf>,
    /// The original as Kantor last read or wrote it, to notice a file changed
    /// by someone else since.
    #[serde(default)]
    pub original_stamp: Option<FileStamp>,
    /// App-specific note on the original's format (Sheets: `csv:59`, `ods`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    /// Content of the original the app can't keep: a restored document must
    /// still not be saved over it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    #[serde(flatten)]
    pub doc: DocInfo,
    /// When the copy was written, in seconds since 1970 (UTC).
    pub saved_at: u64,
    pub kantor_version: String,
    /// The copy's file name inside the slot.
    pub file: String,
    pub len: u64,
    /// FNV-1a 64 of the copy, hex.
    pub checksum: String,
}

/// A recovery copy left by a Kantor that didn't close normally.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    pub manifest: Manifest,
    /// The copy itself.
    pub copy: PathBuf,
}

/// A recovery copy that couldn't be used, set aside in `recovery/damaged/`.
#[derive(Debug, Clone, PartialEq)]
pub struct Damaged {
    pub id: String,
    /// The document's name, if the manifest could be read.
    pub name: Option<String>,
    pub reason: String,
}

/// What [`Store::scan`] found.
#[derive(Debug, Default)]
pub struct Scan {
    /// Newest first.
    pub found: Vec<Found>,
    pub damaged: Vec<Damaged>,
}

/// The recovery store: one directory of slots.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<data dir>/recovery`.
    pub fn default_location() -> Option<Self> {
        Some(Self::new(crate::dirs::data_dir()?.join("recovery")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A new, empty slot owned (locked) by this process.
    pub fn create_slot(&self) -> io::Result<Slot> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(&self.root)?;
        loop {
            let id = format!(
                "{:013}-{}-{}",
                now_millis(),
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let dir = self.root.join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
            let lock = match lock_slot(&dir) {
                Ok(Some(lock)) => lock,
                Ok(None) => continue,
                Err(e) => {
                    let _ = fs::remove_dir_all(&dir);
                    return Err(e);
                }
            };
            return Ok(Slot::new(id, dir, lock, None));
        }
    }

    /// Recovery copies nobody owns (their Kantor crashed or was killed),
    /// newest first. Damaged copies are moved to `recovery/damaged/`; slots
    /// with nothing in them are removed. Slots locked by a running Kantor are
    /// left alone.
    pub fn scan(&self) -> Scan {
        let mut scan = Scan::default();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return scan;
        };
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if !is_slot_id(&id) || !entry.path().is_dir() {
                continue;
            }
            let dir = entry.path();
            let lock = match lock_slot(&dir) {
                Ok(Some(lock)) => lock,
                // In use, or unreadable: not ours to judge.
                Ok(None) | Err(_) => continue,
            };
            match read_slot(&dir) {
                SlotRead::Copy(manifest) => {
                    let copy = dir.join(&manifest.file);
                    scan.found.push(Found { id, manifest, copy });
                    drop(lock);
                }
                SlotRead::Empty => {
                    drop(lock);
                    let _ = fs::remove_dir_all(&dir);
                }
                SlotRead::Damaged { name, reason } => {
                    log::warn!("recovery copy {id} is damaged ({reason}); set aside");
                    drop(lock);
                    let aside = self.root.join(DAMAGED);
                    let _ = fs::create_dir_all(&aside);
                    if let Err(e) = fs::rename(&dir, aside.join(&id)) {
                        log::warn!("setting aside {}: {e}", dir.display());
                    }
                    scan.damaged.push(Damaged { id, name, reason });
                }
            }
        }
        scan.found
            .sort_by(|a, b| b.manifest.saved_at.cmp(&a.manifest.saved_at));
        scan
    }

    /// Take over an ownerless slot found by [`Self::scan`] (to restore it).
    /// Fails if another Kantor took it first or the copy is gone.
    pub fn claim(&self, id: &str) -> io::Result<Slot> {
        if !is_slot_id(id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a slot"));
        }
        let dir = self.root.join(id);
        let Some(lock) = lock_slot(&dir)? else {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another Kantor window has this document",
            ));
        };
        match read_slot(&dir) {
            SlotRead::Copy(manifest) => Ok(Slot::new(id.to_owned(), dir, lock, Some(manifest))),
            SlotRead::Empty => Err(io::Error::new(io::ErrorKind::NotFound, "no copy")),
            SlotRead::Damaged { reason, .. } => {
                Err(io::Error::new(io::ErrorKind::InvalidData, reason))
            }
        }
    }

    /// Delete an ownerless slot (the user discarded its copy).
    pub fn discard(&self, id: &str) -> io::Result<()> {
        let dir = self.root.join(id);
        match lock_slot(&dir)? {
            Some(lock) => Slot::new(id.to_owned(), dir, lock, None).remove(),
            None => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another Kantor window has this document",
            )),
        }
    }
}

/// Orders writes to a slot (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ticket(u64);

/// A slot owned by this process. Dropping it keeps its copy on disk (a crash
/// unwinding must not delete it); [`Slot::remove`] deletes it.
#[derive(Debug)]
pub struct Slot {
    id: String,
    dir: PathBuf,
    lock: Option<File>,
    next: AtomicU64,
    cleared_below: AtomicU64,
    current: Mutex<Option<Current>>,
}

#[derive(Debug)]
struct Current {
    ticket: u64,
    file: String,
}

impl Slot {
    fn new(id: String, dir: PathBuf, lock: File, existing: Option<Manifest>) -> Self {
        Self {
            id,
            dir,
            lock: Some(lock),
            // Ticket 0 is the copy the slot was claimed with.
            next: AtomicU64::new(1),
            cleared_below: AtomicU64::new(0),
            current: Mutex::new(existing.map(|m| Current {
                ticket: 0,
                file: m.file,
            })),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Number the next write; take it when the document is snapshotted.
    pub fn ticket(&self) -> Ticket {
        Ticket(self.next.fetch_add(1, Ordering::SeqCst))
    }

    fn current(&self) -> MutexGuard<'_, Option<Current>> {
        self.current.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Write `bytes` as the slot's copy of `doc`. Returns `Ok(false)` when
    /// the write is stale (a newer copy exists, or the slot was cleared after
    /// `ticket` was taken) and nothing was written. On error (disk full…)
    /// the previous copy stays as it was.
    pub fn write(&self, ticket: Ticket, doc: &DocInfo, bytes: &[u8]) -> io::Result<bool> {
        let mut current = self.current();
        if ticket.0 < self.cleared_below.load(Ordering::SeqCst)
            || current.as_ref().is_some_and(|c| c.ticket > ticket.0)
        {
            return Ok(false);
        }
        let file = format!("copy-{}.{}", ticket.0, doc.app.extension());
        let path = self.dir.join(&file);
        write_atomic(&path, bytes)?;
        let manifest = Manifest {
            format: FORMAT,
            doc: doc.clone(),
            saved_at: now_millis() / 1000,
            kantor_version: env!("CARGO_PKG_VERSION").to_owned(),
            file: file.clone(),
            len: bytes.len() as u64,
            checksum: checksum(bytes),
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
        if let Err(e) = write_atomic(&self.dir.join(MANIFEST), &json) {
            if current.as_ref().is_none_or(|c| c.file != file) {
                let _ = fs::remove_file(&path);
            }
            return Err(e);
        }
        if let Some(old) = current.replace(Current {
            ticket: ticket.0,
            file: file.clone(),
        }) && old.file != file
        {
            let _ = fs::remove_file(self.dir.join(old.file));
        }
        Ok(true)
    }

    /// The document was saved or discarded: writes snapshotted before now
    /// will be skipped. Returns what to pass to [`Self::delete_cleared`]
    /// (which may wait for a write in progress, so can run in the background).
    pub fn mark_cleared(&self) -> u64 {
        let upto = self.next.load(Ordering::SeqCst);
        self.cleared_below.fetch_max(upto, Ordering::SeqCst);
        upto
    }

    /// Delete the copy if it was snapshotted before `upto`.
    pub fn delete_cleared(&self, upto: u64) -> io::Result<()> {
        let mut current = self.current();
        if let Some(c) = current.as_ref()
            && c.ticket < upto
        {
            remove_if_exists(&self.dir.join(MANIFEST))?;
            remove_if_exists(&self.dir.join(&c.file))?;
            *current = None;
        }
        Ok(())
    }

    /// [`Self::mark_cleared`] and [`Self::delete_cleared`] together.
    pub fn clear(&self) -> io::Result<()> {
        let upto = self.mark_cleared();
        self.delete_cleared(upto)
    }

    /// Whether the slot holds a copy now.
    pub fn has_copy(&self) -> bool {
        self.current().is_some()
    }

    /// Delete the slot and its copy.
    pub fn remove(mut self) -> io::Result<()> {
        self.mark_cleared();
        {
            let _current = self.current();
            for entry in fs::read_dir(&self.dir)?.flatten() {
                if entry.file_name() != LOCK {
                    remove_if_exists(&entry.path())?;
                }
            }
        }
        // Windows can't delete a file that is open.
        drop(self.lock.take());
        match fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

enum SlotRead {
    Copy(Manifest),
    Empty,
    Damaged {
        name: Option<String>,
        reason: String,
    },
}

/// Read and check a slot's manifest and copy.
fn read_slot(dir: &Path) -> SlotRead {
    let text = match fs::read(dir.join(MANIFEST)) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // A copy without a manifest: a crash between the two renames.
            // Its original is unknown, but it's still the user's work.
            let copy = fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with("copy-") && !name.ends_with(".tmp")
            });
            return if copy {
                SlotRead::Damaged {
                    name: None,
                    reason: "the copy has no manifest".into(),
                }
            } else {
                SlotRead::Empty
            };
        }
        Err(e) => {
            return SlotRead::Damaged {
                name: None,
                reason: format!("manifest unreadable: {e}"),
            };
        }
    };
    let manifest: Manifest = match serde_json::from_slice(&text) {
        Ok(m) => m,
        Err(e) => {
            return SlotRead::Damaged {
                name: None,
                reason: format!("manifest corrupt: {e}"),
            };
        }
    };
    let name = Some(manifest.doc.name.clone());
    let damaged = |reason: String| SlotRead::Damaged {
        name: name.clone(),
        reason,
    };
    if manifest.format > FORMAT {
        return damaged(format!(
            "written by a newer Kantor ({})",
            manifest.kantor_version
        ));
    }
    if !is_plain_file_name(&manifest.file) {
        return damaged("manifest names a file outside the slot".into());
    }
    let bytes = match fs::read(dir.join(&manifest.file)) {
        Ok(b) => b,
        Err(e) => return damaged(format!("copy unreadable: {e}")),
    };
    if bytes.len() as u64 != manifest.len {
        return damaged(format!(
            "copy is {} bytes, expected {}",
            bytes.len(),
            manifest.len
        ));
    }
    if checksum(&bytes) != manifest.checksum {
        return damaged("copy checksum doesn't match".into());
    }
    SlotRead::Copy(manifest)
}

/// Open and lock a slot's lock file; `None` if another process (or another
/// slot handle) holds it.
fn lock_slot(dir: &Path) -> io::Result<Option<File>> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK))?;
    match file.try_lock() {
        Ok(()) => {
            // For people looking at the folder: who has it.
            let _ = file.set_len(0);
            let _ = writeln!(file, "{}", std::process::id());
            Ok(Some(file))
        }
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

fn is_slot_id(id: &str) -> bool {
    !id.is_empty()
        && id != DAMAGED
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && name != MANIFEST
        && name != LOCK
}

/// Write via a temporary file in the same directory, synced, then renamed.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = path.with_file_name(format!(".{}.tmp", file_name.to_string_lossy()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// FNV-1a 64 as 16 hex digits: catches truncated and scribbled copies.
pub fn checksum(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn doc(name: &str, original: Option<&Path>) -> DocInfo {
        DocInfo {
            app: App::Sheets,
            name: name.into(),
            original: original.map(Path::to_path_buf),
            original_stamp: original.and_then(FileStamp::of),
            source_format: Some("xlsx".into()),
            unsupported: Vec::new(),
        }
    }

    #[test]
    fn write_manifest_and_find_after_crash() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("recovery"));
        let original = tmp.path().join("budget.xlsx");
        fs::write(&original, b"original bytes").unwrap();

        let slot = store.create_slot().unwrap();
        let t = slot.ticket();
        assert!(
            slot.write(t, &doc("budget.xlsx", Some(&original)), b"copy 1")
                .unwrap()
        );
        // While this process holds the slot, nobody else sees it.
        assert!(store.scan().found.is_empty());

        // "Crash": the slot is dropped without being removed.
        let id = slot.id().to_owned();
        drop(slot);
        let scan = store.scan();
        assert!(scan.damaged.is_empty());
        let [found] = scan.found.as_slice() else {
            panic!("expected one copy: {:?}", scan.found)
        };
        assert_eq!(found.id, id);
        let m = &found.manifest;
        assert_eq!(m.format, FORMAT);
        assert_eq!(m.doc.app, App::Sheets);
        assert_eq!(m.doc.name, "budget.xlsx");
        // The restored document points at the original, not at the copy.
        assert_eq!(m.doc.original.as_deref(), Some(original.as_path()));
        assert_eq!(m.doc.original_stamp, FileStamp::of(&original));
        assert_eq!(m.kantor_version, env!("CARGO_PKG_VERSION"));
        assert!(m.saved_at > 1_700_000_000);
        assert_eq!(fs::read(&found.copy).unwrap(), b"copy 1");
        assert!(found.copy.starts_with(store.root()));
        // The original was never touched.
        assert_eq!(fs::read(&original).unwrap(), b"original bytes");
    }

    #[test]
    fn newer_copy_replaces_older_and_stale_writes_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let slot = store.create_slot().unwrap();
        let d = doc("Untitled 2.xlsx", None);
        let old = slot.ticket();
        let new = slot.ticket();
        assert!(slot.write(new, &d, b"new").unwrap());
        assert!(
            !slot.write(old, &d, b"old").unwrap(),
            "older snapshot skipped"
        );
        let files: Vec<_> = fs::read_dir(slot.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 3, "lock, manifest, one copy: {files:?}");
        drop(slot);
        let found = store.scan().found;
        assert_eq!(fs::read(&found[0].copy).unwrap(), b"new");
        assert_eq!(found[0].manifest.doc.original, None);
    }

    #[test]
    fn cleared_on_save_and_removed_on_close() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let slot = store.create_slot().unwrap();
        let d = doc("a.xlsx", None);
        let snapshotted = slot.ticket();
        // Saved while the copy was being made: the late write is dropped.
        let upto = slot.mark_cleared();
        assert!(!slot.write(snapshotted, &d, b"late").unwrap());
        slot.delete_cleared(upto).unwrap();
        assert!(!slot.has_copy());

        // Edited again after the save: a copy, then cleared by a save.
        assert!(slot.write(slot.ticket(), &d, b"again").unwrap());
        assert!(slot.has_copy());
        slot.clear().unwrap();
        assert!(!slot.has_copy());
        let dir = slot.dir().to_path_buf();
        assert!(dir.join(LOCK).exists());
        assert!(!dir.join(MANIFEST).exists());

        // A clear then a newer write: the newer copy stays.
        let upto = slot.mark_cleared();
        assert!(slot.write(slot.ticket(), &d, b"newer").unwrap());
        slot.delete_cleared(upto).unwrap();
        assert!(slot.has_copy());

        slot.remove().unwrap();
        assert!(!dir.exists());
        assert!(store.scan().found.is_empty());
    }

    #[test]
    fn empty_slots_of_dead_processes_are_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let slot = store.create_slot().unwrap();
        let dir = slot.dir().to_path_buf();
        drop(slot);
        let scan = store.scan();
        assert!(scan.found.is_empty() && scan.damaged.is_empty());
        assert!(!dir.exists());
    }

    #[test]
    fn corrupt_copies_are_set_aside_not_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let d = doc("report.docx", None);

        let truncated = store.create_slot().unwrap();
        truncated
            .write(truncated.ticket(), &d, b"full copy")
            .unwrap();
        let file = truncated.current().as_ref().unwrap().file.clone();
        fs::write(truncated.dir().join(&file), b"full").unwrap();

        let scribbled = store.create_slot().unwrap();
        scribbled.write(scribbled.ticket(), &d, b"abcd").unwrap();
        let file2 = scribbled.current().as_ref().unwrap().file.clone();
        fs::write(scribbled.dir().join(&file2), b"abce").unwrap();

        let bad_json = store.create_slot().unwrap();
        fs::write(bad_json.dir().join(MANIFEST), b"{\"format\": 1, \"app\"").unwrap();

        let escaping = store.create_slot().unwrap();
        escaping.write(escaping.ticket(), &d, b"x").unwrap();
        let mut m: Manifest =
            serde_json::from_slice(&fs::read(escaping.dir().join(MANIFEST)).unwrap()).unwrap();
        m.file = "../../etc/passwd".into();
        fs::write(
            escaping.dir().join(MANIFEST),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();

        let orphan_copy = store.create_slot().unwrap();
        fs::write(orphan_copy.dir().join("copy-3.xlsx"), b"no manifest").unwrap();

        let good = store.create_slot().unwrap();
        good.write(good.ticket(), &d, b"fine").unwrap();

        let ids: Vec<String> = [&truncated, &scribbled, &bad_json, &escaping, &orphan_copy]
            .iter()
            .map(|s| s.id().to_owned())
            .collect();
        let good_id = good.id().to_owned();
        drop((truncated, scribbled, bad_json, escaping, orphan_copy, good));

        let scan = store.scan();
        assert_eq!(scan.found.len(), 1);
        assert_eq!(scan.found[0].id, good_id);
        assert_eq!(scan.damaged.len(), 5, "{:?}", scan.damaged);
        for id in &ids {
            assert!(
                tmp.path().join(DAMAGED).join(id).is_dir(),
                "{id} kept in damaged/"
            );
            assert!(!tmp.path().join(id).exists());
        }
        let truncated = scan.damaged.iter().find(|d| d.id == ids[0]).unwrap();
        assert_eq!(truncated.name.as_deref(), Some("report.docx"));
        assert!(truncated.reason.contains("bytes"), "{}", truncated.reason);
        // Set aside once: the next launch doesn't report them again.
        let again = store.scan();
        assert!(again.damaged.is_empty());
        assert_eq!(again.found.len(), 1);
    }

    #[test]
    fn claim_and_discard_respect_other_owners() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let d = doc("a.xlsx", None);
        let slot = store.create_slot().unwrap();
        slot.write(slot.ticket(), &d, b"v1").unwrap();
        let id = slot.id().to_owned();
        // Another window holds it: no claim, no discard.
        assert_eq!(
            store.claim(&id).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(store.discard(&id).is_err());
        drop(slot);

        // Restored: the claimed slot keeps the copy until saved or discarded.
        let claimed = store.claim(&id).unwrap();
        assert!(claimed.has_copy());
        assert!(store.claim(&id).is_err(), "only one owner");
        assert!(store.scan().found.is_empty(), "owned slots aren't offered");
        // A newer copy replaces the restored one.
        claimed.write(claimed.ticket(), &d, b"v2").unwrap();
        drop(claimed);
        let found = store.scan().found;
        assert_eq!(fs::read(&found[0].copy).unwrap(), b"v2");

        store.discard(&id).unwrap();
        assert!(!tmp.path().join(&id).exists());
        assert!(store.claim("../x").is_err());
    }

    #[test]
    fn concurrent_writers() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        // Several "processes" each with their own slot, and several threads
        // racing on one slot.
        let shared = Arc::new(store.create_slot().unwrap());
        let mut handles = Vec::new();
        for i in 0..8u8 {
            let store = store.clone();
            let shared = shared.clone();
            handles.push(std::thread::spawn(move || {
                let own = store.create_slot().unwrap();
                let d = doc(&format!("doc {i}.xlsx"), None);
                for round in 0..20u8 {
                    own.write(own.ticket(), &d, &[i, round]).unwrap();
                    let t = shared.ticket();
                    shared
                        .write(t, &doc("shared.xlsx", None), &[i, round, 0xAA])
                        .unwrap();
                }
                own.id().to_owned()
            }));
        }
        let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let last = shared.next.load(Ordering::SeqCst) - 1;
        let current = shared.current().as_ref().unwrap().ticket;
        assert_eq!(current, last, "the newest snapshot wins");
        let shared_id = shared.id().to_owned();
        drop(shared);

        let scan = store.scan();
        assert!(scan.damaged.is_empty(), "{:?}", scan.damaged);
        assert_eq!(scan.found.len(), 9);
        for found in &scan.found {
            let bytes = fs::read(&found.copy).unwrap();
            if found.id == shared_id {
                assert_eq!(bytes.len(), 3);
            } else {
                assert!(ids.contains(&found.id));
                assert_eq!(bytes[1], 19, "last round");
            }
            let entries = fs::read_dir(found.copy.parent().unwrap()).unwrap().count();
            assert_eq!(entries, 3, "no stray copies or temp files");
        }
    }

    #[test]
    fn write_errors_keep_the_previous_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let slot = store.create_slot().unwrap();
        let d = doc("a.xlsx", None);
        slot.write(slot.ticket(), &d, b"good").unwrap();
        // Stand-in for a full disk: the copy's temp name is taken by a directory.
        let t = slot.ticket();
        let blocker = slot.dir().join(format!(".copy-{}.xlsx.tmp", t.0));
        fs::create_dir(&blocker).unwrap();
        assert!(slot.write(t, &d, b"bad").is_err());
        fs::remove_dir(&blocker).unwrap();
        drop(slot);
        let found = store.scan().found;
        assert_eq!(fs::read(&found[0].copy).unwrap(), b"good");
    }

    #[test]
    fn checksum_is_stable() {
        assert_eq!(checksum(b""), "cbf29ce484222325");
        assert_eq!(checksum(b"a"), "af63dc4c8601ec8c");
    }
}
