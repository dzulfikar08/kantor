//! The local copies of cloud files and what Kantor knows about them.
//!
//! ```text
//! <root>/index.json              cache path → CacheEntry
//! <root>/locks.json              WebDAV locks this computer holds (crash cleanup)
//! <root>/<account>/<remote path> the working copy the app opens and saves
//! ```
//!
//! The app saves into the working copy like any local file; a save is
//! noticed by its [`Stamp`] (size and mtime) differing from the one recorded
//! when the copy last matched the server.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::LockToken;

/// Size and modification time of a file: changes when the app saves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub len: u64,
    pub mtime_ns: u128,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        let mtime_ns = m
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            len: m.len(),
            mtime_ns,
        })
    }
}

/// One cloud file with a working copy on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The account's id (see the app's cloud settings).
    pub account: String,
    /// Path under the account's WebDAV root, `/`-separated, unencoded.
    pub remote: String,
    /// The working copy.
    pub local: PathBuf,
    /// The server's ETag the working copy is based on (`None`: not on the
    /// server yet).
    pub etag: Option<String>,
    /// The working copy's stamp when it last matched `etag`.
    pub synced: Option<Stamp>,
    /// The server's `getlastmodified` and size for that version.
    #[serde(default)]
    pub modified: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl CacheEntry {
    /// The server's copy is still the version this one is based on.
    ///
    /// Nextcloud's `files_lock` gives a file a new ETag when it is locked
    /// or unlocked, without changing it (measured on Nextcloud 35): so a
    /// different ETag with the same modification time and size counts as
    /// the same version. Servers without `getlastmodified` fall back to the
    /// ETag alone.
    pub fn same_version(&self, server: &crate::Entry) -> bool {
        if server.etag.is_some() && server.etag == self.etag {
            return true;
        }
        match (
            &self.modified,
            &server.last_modified,
            self.size,
            server.size,
        ) {
            (Some(a), Some(b), Some(x), Some(y)) => a == b && x == y,
            _ => false,
        }
    }

    /// Take the server's current version as the base (after a download,
    /// an upload, or a metadata-only change).
    pub fn rebase(&mut self, server: &crate::Entry) {
        self.etag = server.etag.clone();
        self.modified = server.last_modified.clone();
        self.size = server.size;
    }

    /// The working copy was saved since it last matched the server.
    pub fn has_local_changes(&self) -> bool {
        Stamp::of(&self.local) != self.synced
    }

    /// The file name (`Laporan Q3.xlsx`).
    pub fn name(&self) -> &str {
        self.remote.rsplit('/').next().unwrap_or(&self.remote)
    }

    /// The folder holding the file (`Keuangan/2026`, or `""` at the root).
    pub fn folder(&self) -> &str {
        self.remote
            .trim_end_matches('/')
            .rsplit_once('/')
            .map_or("", |(dir, _)| dir)
    }
}

/// A lock this computer took, kept until released so a crash can be
/// cleaned up on the next start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldLock {
    pub account: String,
    pub remote: String,
    pub token: String,
}

impl HeldLock {
    pub fn token(&self) -> LockToken {
        LockToken(self.token.clone())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    entries: Vec<CacheEntry>,
}

/// Index and lock ledger writes are read-modify-write; one at a time.
static WRITE: Mutex<()> = Mutex::new(());

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the working copy of `remote` on `account` lives.
    pub fn local_path(&self, account: &str, remote: &str) -> PathBuf {
        let mut p = self.root.join(safe_segment(account));
        for seg in remote.split('/').filter(|s| !s.is_empty()) {
            p.push(safe_segment(seg));
        }
        p
    }

    /// Whether `path` is a working copy in this cache.
    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn locks_path(&self) -> PathBuf {
        self.root.join("locks.json")
    }

    fn load_index(&self) -> Index {
        read_json(&self.index_path()).unwrap_or_default()
    }

    pub fn entries(&self) -> Vec<CacheEntry> {
        self.load_index().entries
    }

    /// The entry for the working copy at `local`.
    pub fn find(&self, local: &Path) -> Option<CacheEntry> {
        self.entries().into_iter().find(|e| e.local == local)
    }

    /// The entry for `remote` on `account`.
    pub fn find_remote(&self, account: &str, remote: &str) -> Option<CacheEntry> {
        self.entries()
            .into_iter()
            .find(|e| e.account == account && e.remote == remote)
    }

    /// Add or replace (by working copy path) an entry.
    pub fn save_entry(&self, entry: &CacheEntry) -> std::io::Result<()> {
        let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let mut index = self.load_index();
        index.entries.retain(|e| e.local != entry.local);
        index.entries.push(entry.clone());
        write_json(&self.index_path(), &index)
    }

    pub fn held_locks(&self) -> Vec<HeldLock> {
        read_json(&self.locks_path()).unwrap_or_default()
    }

    pub fn record_lock(&self, lock: HeldLock) -> std::io::Result<()> {
        let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let mut locks: Vec<HeldLock> = read_json(&self.locks_path()).unwrap_or_default();
        locks.retain(|l| !(l.account == lock.account && l.remote == lock.remote));
        locks.push(lock);
        write_json(&self.locks_path(), &locks)
    }

    pub fn forget_lock(&self, account: &str, remote: &str) -> std::io::Result<()> {
        let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let mut locks: Vec<HeldLock> = read_json(&self.locks_path()).unwrap_or_default();
        let before = locks.len();
        locks.retain(|l| !(l.account == account && l.remote == remote));
        if locks.len() == before {
            return Ok(());
        }
        write_json(&self.locks_path(), &locks)
    }
}

/// Write `bytes` to `path` through a temporary file in the same folder, so
/// the app never sees a half-written working copy.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".kantor-tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("{}: {e}", path.display());
            None
        }
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    write_atomic(path, text.as_bytes())
}

/// A path segment that is safe as a file name on every OS: `..`, separators
/// and characters Windows forbids are replaced.
fn safe_segment(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if out == "." || out == ".." || out.is_empty() {
        out = format!("_{out}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_stay_inside_the_cache() {
        let c = Cache::new("/c");
        assert_eq!(
            c.local_path("acc1", "Keuangan/Laporan Q3.xlsx"),
            PathBuf::from("/c/acc1/Keuangan/Laporan Q3.xlsx")
        );
        let p = c.local_path("acc1", "../../etc/a:b.xlsx");
        assert!(p.starts_with("/c/acc1"));
        assert_eq!(p, PathBuf::from("/c/acc1/_../_../etc/a_b.xlsx"));
        assert!(c.contains(&p));
    }

    #[test]
    fn index_round_trip_and_local_changes() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::new(dir.path());
        let local = c.local_path("a", "x/Doc.docx");
        write_atomic(&local, b"v1").unwrap();
        let mut e = CacheEntry {
            account: "a".into(),
            remote: "x/Doc.docx".into(),
            local: local.clone(),
            etag: Some("\"1\"".into()),
            synced: Stamp::of(&local),
            ..Default::default()
        };
        assert!(!e.has_local_changes());
        assert_eq!(e.name(), "Doc.docx");
        assert_eq!(e.folder(), "x");
        c.save_entry(&e).unwrap();
        assert_eq!(c.find(&local), Some(e.clone()));
        assert_eq!(c.find_remote("a", "x/Doc.docx"), Some(e.clone()));
        std::fs::write(&local, b"v2 longer").unwrap();
        assert!(e.has_local_changes());
        e.etag = Some("\"2\"".into());
        c.save_entry(&e).unwrap();
        assert_eq!(c.entries().len(), 1, "replaced, not duplicated");
    }

    #[test]
    fn lock_only_etag_changes_are_the_same_version() {
        let base = CacheEntry {
            etag: Some("\"a1\"".into()),
            modified: Some("Fri, 13 Nov 2026 16:39:04 GMT".into()),
            size: Some(5),
            ..Default::default()
        };
        let mut server = crate::Entry {
            href: "/f".into(),
            name: "f".into(),
            is_dir: false,
            size: Some(5),
            etag: Some("\"a1\"".into()),
            last_modified: base.modified.clone(),
            content_type: None,
        };
        assert!(base.same_version(&server));
        // Locked: new ETag, same file.
        server.etag = Some("\"6af73d2b5e479\"".into());
        assert!(base.same_version(&server));
        // Really changed.
        server.last_modified = Some("Fri, 13 Nov 2026 16:40:00 GMT".into());
        assert!(!base.same_version(&server));
        server.last_modified = base.modified.clone();
        server.size = Some(6);
        assert!(!base.same_version(&server));
        // No dates: the ETag decides.
        let bare = CacheEntry {
            etag: Some("\"a1\"".into()),
            ..Default::default()
        };
        server.size = Some(5);
        assert!(!bare.same_version(&server));
        let mut rebased = bare.clone();
        rebased.rebase(&server);
        assert!(rebased.same_version(&server));
    }

    #[test]
    fn lock_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::new(dir.path());
        let l = HeldLock {
            account: "a".into(),
            remote: "f.xlsx".into(),
            token: "opaquelocktoken:1".into(),
        };
        c.record_lock(l.clone()).unwrap();
        c.record_lock(l.clone()).unwrap();
        assert_eq!(c.held_locks(), vec![l]);
        c.forget_lock("a", "f.xlsx").unwrap();
        assert!(c.held_locks().is_empty());
    }
}
