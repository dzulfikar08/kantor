//! The bookmark list (`bookmarks.json`) and the rules for using it. The
//! operating system's part (making, resolving and starting a bookmark) is
//! behind [`Os`], so the rules are tested on every platform.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

/// Bookmarks kept, newest first; older ones are dropped.
pub const MAX: usize = 256;

/// What the system does with bookmarks.
pub trait Os: Sync {
    /// A security-scoped bookmark for `path` (needs access to it now).
    fn create(&self, path: &Path, dir: bool) -> Option<Vec<u8>>;
    /// Resolve `data` and start using it for the rest of this run.
    fn open(&self, data: &[u8]) -> Option<Opened>;
    /// Whether the sandbox lets Kantor use `path` right now.
    fn can_use(&self, path: &Path) -> bool;
}

/// A resolved bookmark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// Where the file or folder is now (it may have been moved).
    pub path: PathBuf,
    /// The bookmark should be made again (the system says so).
    pub stale: bool,
}

/// No sandbox (other platforms): nothing to bookmark.
pub struct NoOs;

impl Os for NoOs {
    fn create(&self, _: &Path, _: bool) -> Option<Vec<u8>> {
        None
    }
    fn open(&self, _: &[u8]) -> Option<Opened> {
        None
    }
    fn can_use(&self, path: &Path) -> bool {
        usable(path)
    }
}

/// Whether `path` can be opened (a file) or listed (a folder).
pub fn usable(path: &Path) -> bool {
    if path.is_dir() {
        std::fs::read_dir(path).is_ok()
    } else {
        std::fs::File::open(path).is_ok()
    }
}

/// One bookmark.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    #[serde(default)]
    pub dir: bool,
    /// The bookmark data, base64.
    pub data: String,
}

#[derive(Default, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    bookmarks: Vec<Entry>,
}

/// The bookmarks and what this run has started using.
#[derive(Default)]
pub struct Store {
    file: Option<PathBuf>,
    entries: Vec<Entry>,
    /// Files and folders in use this run (bookmarks started, or picked).
    open: Vec<PathBuf>,
    /// Paths the system made no bookmark for this run (not tried again).
    failed: Vec<PathBuf>,
}

impl Store {
    /// Read `file` (missing or unreadable: empty).
    pub fn load(file: Option<PathBuf>) -> Self {
        let entries = file
            .as_deref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str::<FileFormat>(&s).ok())
            .map(|f| f.bookmarks)
            .unwrap_or_default();
        Store {
            file,
            entries,
            open: Vec::new(),
            failed: Vec::new(),
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether `path` itself has a bookmark.
    pub fn has(&self, path: &Path) -> bool {
        self.entries.iter().any(|e| e.path == path)
    }

    /// Bookmark `path` unless it has one or failed before this run.
    pub fn remember_once(&mut self, os: &dyn Os, path: &Path) {
        if !self.has(path) && !self.failed.iter().any(|f| f == path) {
            self.remember(os, path);
        }
    }

    /// Bookmark `path` (an absolute path Kantor can use now). `false` when
    /// the system made no bookmark.
    pub fn remember(&mut self, os: &dyn Os, path: &Path) -> bool {
        if !path.is_absolute() {
            return false;
        }
        let dir = path.is_dir();
        let Some(data) = os.create(path, dir) else {
            log::warn!("no bookmark for {}", path.display());
            self.failed.push(path.to_path_buf());
            return false;
        };
        self.put(Entry {
            path: path.to_path_buf(),
            dir,
            data: B64.encode(data),
        });
        self.mark_open(path);
        self.save();
        true
    }

    /// See [`crate::access`].
    pub fn access(&mut self, os: &dyn Os, path: &Path) -> bool {
        if self.open.iter().any(|o| path.starts_with(o)) {
            return true;
        }
        let candidates: Vec<Entry> = path
            .ancestors()
            .filter_map(|a| self.entries.iter().find(|e| e.path == a).cloned())
            .collect();
        let mut changed = false;
        let mut granted = false;
        for entry in candidates {
            let opened = B64
                .decode(&entry.data)
                .ok()
                .and_then(|data| os.open(&data));
            let Some(opened) = opened else {
                // Deleted, or on a disk that's gone: forget it.
                self.entries.retain(|e| e.path != entry.path);
                changed = true;
                continue;
            };
            self.mark_open(&opened.path);
            if opened.stale || opened.path != entry.path {
                if let Some(data) = os.create(&opened.path, entry.dir) {
                    self.entries.retain(|e| e.path != entry.path);
                    self.put(Entry {
                        path: opened.path.clone(),
                        dir: entry.dir,
                        data: B64.encode(data),
                    });
                    changed = true;
                }
            } else {
                // Most recently used first.
                self.put(entry);
                changed = true;
            }
            if path.starts_with(&opened.path) {
                granted = true;
                break;
            }
        }
        if !granted && os.can_use(path) {
            // Picked, opened from Finder or dropped this run.
            self.remember(os, path);
            self.mark_open(path);
            granted = true;
        }
        if changed {
            self.save();
        }
        granted
    }

    fn put(&mut self, entry: Entry) {
        self.entries.retain(|e| e.path != entry.path);
        self.entries.insert(0, entry);
        self.entries.truncate(MAX);
    }

    fn mark_open(&mut self, path: &Path) {
        if !self.open.iter().any(|o| o == path) {
            self.open.push(path.to_path_buf());
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let body = FileFormat {
            version: 1,
            bookmarks: self.entries.clone(),
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let json = serde_json::to_vec_pretty(&body).map_err(std::io::Error::other)?;
            kantor_core::fsx::write_atomic(file, &json)
        };
        if let Err(e) = write() {
            log::warn!("saving {}: {e}", file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// Bookmark data = "<path>" (or "dead:<path>"); `moved` maps old → new.
    #[derive(Default)]
    struct Fake {
        usable: HashSet<PathBuf>,
        stale: bool,
        moved: Vec<(PathBuf, PathBuf)>,
        dead: HashSet<PathBuf>,
        opened: Mutex<Vec<PathBuf>>,
        created: Mutex<Vec<PathBuf>>,
    }

    impl Os for Fake {
        fn create(&self, path: &Path, _: bool) -> Option<Vec<u8>> {
            self.created.lock().unwrap().push(path.to_path_buf());
            Some(path.to_string_lossy().as_bytes().to_vec())
        }
        fn open(&self, data: &[u8]) -> Option<Opened> {
            let p = PathBuf::from(String::from_utf8(data.to_vec()).ok()?);
            if self.dead.contains(&p) {
                return None;
            }
            let now = self
                .moved
                .iter()
                .find(|(a, _)| *a == p)
                .map_or(p.clone(), |(_, b)| b.clone());
            self.opened.lock().unwrap().push(now.clone());
            Some(Opened {
                path: now,
                stale: self.stale,
            })
        }
        fn can_use(&self, path: &Path) -> bool {
            self.usable.contains(path)
        }
    }

    fn p(s: &str) -> PathBuf {
        // Absolute on every platform.
        std::env::temp_dir().join("kantor-sandbox-test").join(s)
    }

    #[test]
    fn picked_files_are_remembered_and_reopened_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bookmarks.json");
        let doc = p("a/report.docx");
        let os = Fake {
            usable: [doc.clone()].into(),
            ..Fake::default()
        };
        let mut s = Store::load(Some(file.clone()));
        assert!(s.access(&os, &doc), "picked this run");
        assert!(s.has(&doc));

        // Next run: the sandbox no longer allows it; the bookmark does.
        let os = Fake::default();
        let mut s = Store::load(Some(file.clone()));
        assert_eq!(s.entries().len(), 1);
        assert!(s.access(&os, &doc));
        assert_eq!(*os.opened.lock().unwrap(), vec![doc.clone()]);
        // Started once per run.
        assert!(s.access(&os, &doc));
        assert_eq!(os.opened.lock().unwrap().len(), 1);
        // Never seen, not allowed.
        assert!(!s.access(&os, &p("a/other.docx")));
    }

    #[test]
    fn a_folder_bookmark_covers_its_files() {
        let os = Fake::default();
        let mut s = Store::default();
        let folder = p("batch/in");
        s.put(Entry {
            path: folder.clone(),
            dir: true,
            data: B64.encode(folder.to_string_lossy().as_bytes()),
        });
        assert!(s.access(&os, &folder.join("x/1.pdf")));
        assert!(s.access(&os, &folder.join("2.pdf")));
        assert_eq!(os.opened.lock().unwrap().len(), 1);
        assert!(!s.access(&os, &p("batch/out/1.pdf")));
    }

    #[test]
    fn stale_moved_and_dead_bookmarks() {
        let a = p("a.xlsx");
        let b = p("b.xlsx");
        let gone = p("gone.xlsx");
        let os = Fake {
            stale: true,
            moved: vec![(a.clone(), p("moved/a.xlsx"))],
            dead: [gone.clone()].into(),
            ..Fake::default()
        };
        let mut s = Store::default();
        for f in [&a, &b, &gone] {
            s.put(Entry {
                path: f.clone(),
                dir: false,
                data: B64.encode(f.to_string_lossy().as_bytes()),
            });
        }
        // Stale: made again.
        assert!(s.access(&os, &b));
        assert!(os.created.lock().unwrap().contains(&b));
        // Moved: the old path isn't granted, the bookmark now names the new one.
        assert!(!s.access(&os, &a));
        assert!(s.has(&p("moved/a.xlsx")) && !s.has(&a));
        // Dead: forgotten.
        assert!(!s.access(&os, &gone));
        assert!(!s.has(&gone));
    }

    #[test]
    fn newest_first_and_capped() {
        let os = Fake::default();
        let mut s = Store::default();
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX + 10 {
            s.remember(&os, &dir.path().join(format!("{i}.docx")));
        }
        assert_eq!(s.entries().len(), MAX);
        assert_eq!(
            s.entries()[0].path,
            dir.path().join(format!("{}.docx", MAX + 9))
        );
        // Relative paths are never bookmarked.
        assert!(!s.remember(&os, Path::new("rel.docx")));
    }

    #[test]
    fn file_format() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state/bookmarks.json");
        let os = Fake::default();
        let mut s = Store::load(Some(file.clone()));
        s.remember(&os, &p("x.pptx"));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["bookmarks"][0]["dir"], false);
        assert_eq!(
            B64.decode(v["bookmarks"][0]["data"].as_str().unwrap())
                .unwrap(),
            p("x.pptx").to_string_lossy().as_bytes()
        );
        // A broken file starts empty instead of failing.
        std::fs::write(&file, "{").unwrap();
        assert!(Store::load(Some(file)).entries().is_empty());
    }

    #[test]
    fn nothing_to_do_outside_the_sandbox() {
        if !crate::ACTIVE {
            assert!(crate::access(Path::new("/no/such/file")));
            assert!(crate::access_dir_of(Path::new("/no/such/file")));
        }
    }
}
