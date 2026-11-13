//! The sync and conflict rules for one open cloud document, as a pure state
//! machine: the app feeds it events (a local save, an upload's outcome, the
//! server coming back) and does what the returned [`Action`] says. The
//! network work itself is in [`crate::session`].

use crate::LockToken;
use crate::cache::{CacheEntry, Stamp};

/// The document's WebDAV lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockState {
    /// No lock (not taken yet, released, or the server was unreachable).
    None,
    /// We hold it; PUTs carry the token.
    Held(LockToken),
    /// Someone else holds it: the document is read-only here.
    LockedBy(String),
    /// The server doesn't do locks (or refused for another reason); saving
    /// still works, guarded by ETags.
    Unsupported(String),
}

/// Where the working copy stands against the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncState {
    /// The working copy matches the server.
    Synced,
    /// Saved here, not uploaded yet.
    LocalChanges,
    Uploading {
        sent: u64,
        total: u64,
    },
    /// The server couldn't be reached; local saves wait for "Sync now".
    Offline,
    /// The server's copy changed since this one was opened (412, or the
    /// ETag re-check before a save). Resolved by the conflict dialog.
    Conflict {
        server_etag: Option<String>,
    },
    /// The last upload failed for another reason.
    Failed(String),
}

/// How an upload ended (see `session::upload`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadOutcome {
    /// The server has the working copy as of `stamp`, as version `server`
    /// (ETag, modification time, size).
    Synced {
        server: Option<crate::Entry>,
        stamp: Option<Stamp>,
        /// Our lock turned out to be gone; take a new one.
        lock_lost: bool,
        /// SHA-256 of the uploaded bytes.
        sha256: Option<String>,
    },
    /// The server's copy moved on (or was deleted: `server_etag: None`).
    Conflict {
        server_etag: Option<String>,
    },
    /// The server couldn't be reached.
    Offline(String),
    /// Someone else locked the file.
    LockedBy(String),
    /// The file is locked but the lock is ours: the server is still busy
    /// with an earlier, aborted upload (Nextcloud keeps its own file lock
    /// while it finishes one). Try again shortly.
    Busy,
    Failed(String),
}

/// What the app should do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Start an upload of the working copy (respecting ETags).
    Upload,
    /// Show the conflict dialog.
    ShowConflict,
    /// Tell the user the document is read-only (locked by someone else).
    NotifyReadOnly,
    /// Retry the upload after a short wait.
    RetryLater,
}

/// One open cloud document.
#[derive(Clone, Debug)]
pub struct DocSync {
    pub entry: CacheEntry,
    pub lock: LockState,
    pub state: SyncState,
    /// A local save happened while an upload was running.
    pending_again: bool,
}

impl DocSync {
    /// A document just downloaded (in sync) or opened from the working copy.
    pub fn new(entry: CacheEntry, lock: LockState) -> Self {
        let state = if entry.has_local_changes() {
            SyncState::LocalChanges
        } else {
            SyncState::Synced
        };
        Self {
            entry,
            lock,
            state,
            pending_again: false,
        }
    }

    /// Opened from the working copy because the server was unreachable.
    pub fn offline(entry: CacheEntry) -> Self {
        Self {
            entry,
            lock: LockState::None,
            state: SyncState::Offline,
            pending_again: false,
        }
    }

    pub fn read_only_by(&self) -> Option<&str> {
        match &self.lock {
            LockState::LockedBy(who) => Some(who),
            _ => None,
        }
    }

    pub fn lock_token(&self) -> Option<&LockToken> {
        match &self.lock {
            LockState::Held(t) => Some(t),
            _ => None,
        }
    }

    pub fn is_uploading(&self) -> bool {
        matches!(self.state, SyncState::Uploading { .. })
    }

    /// Local saves not on the server yet.
    pub fn unsynced(&self) -> bool {
        self.entry.has_local_changes()
    }

    /// The app saved the working copy.
    pub fn on_local_save(&mut self) -> Action {
        if self.read_only_by().is_some() {
            self.state = SyncState::LocalChanges;
            return Action::NotifyReadOnly;
        }
        match self.state {
            SyncState::Uploading { .. } => {
                self.pending_again = true;
                Action::None
            }
            SyncState::Conflict { .. } => Action::ShowConflict,
            // Keep editing offline; "Sync now" uploads.
            SyncState::Offline => Action::None,
            _ => {
                self.state = SyncState::LocalChanges;
                Action::Upload
            }
        }
    }

    pub fn begin_upload(&mut self, total: u64) {
        self.pending_again = false;
        self.state = SyncState::Uploading { sent: 0, total };
    }

    pub fn on_progress(&mut self, sent: u64, total: u64) {
        if self.is_uploading() {
            self.state = SyncState::Uploading { sent, total };
        }
    }

    pub fn on_upload(&mut self, outcome: UploadOutcome) -> Action {
        let again = std::mem::take(&mut self.pending_again);
        match outcome {
            UploadOutcome::Synced {
                server,
                stamp,
                lock_lost,
                sha256,
            } => {
                if lock_lost {
                    self.lock = LockState::None;
                }
                self.entry.sha256 = sha256;
                match server {
                    Some(s) => self.entry.rebase(&s),
                    None => self.entry.etag = None,
                }
                self.entry.synced = stamp;
                if again || self.entry.has_local_changes() {
                    self.state = SyncState::LocalChanges;
                    Action::Upload
                } else {
                    self.state = SyncState::Synced;
                    Action::None
                }
            }
            UploadOutcome::Conflict { server_etag } => {
                self.state = SyncState::Conflict { server_etag };
                Action::ShowConflict
            }
            UploadOutcome::Offline(_) => {
                self.state = SyncState::Offline;
                Action::None
            }
            UploadOutcome::LockedBy(who) => {
                self.lock = LockState::LockedBy(who);
                self.state = SyncState::LocalChanges;
                Action::NotifyReadOnly
            }
            UploadOutcome::Busy => {
                self.state = SyncState::LocalChanges;
                Action::RetryLater
            }
            UploadOutcome::Failed(msg) => {
                self.state = SyncState::Failed(msg);
                Action::None
            }
        }
    }

    /// The server answers again (or the user chose "Sync now").
    pub fn on_reconnect(&mut self) -> Action {
        match self.state {
            SyncState::Uploading { .. } | SyncState::Conflict { .. } => Action::None,
            _ if self.read_only_by().is_some() => Action::None,
            _ if self.entry.has_local_changes() => {
                self.state = SyncState::LocalChanges;
                Action::Upload
            }
            _ => {
                self.state = SyncState::Synced;
                Action::None
            }
        }
    }

    /// The conflict was resolved by replacing the working copy with the
    /// server's version (reload, or after saving ours as a copy).
    pub fn on_reloaded(&mut self, entry: CacheEntry) {
        self.entry = entry;
        self.state = SyncState::Synced;
        self.pending_again = false;
    }

    /// Short status for the status bar.
    pub fn status(&self) -> Status {
        if let Some(who) = self.read_only_by() {
            return Status::ReadOnly(who.to_owned());
        }
        match &self.state {
            SyncState::Synced => Status::Synced,
            SyncState::LocalChanges => Status::Pending,
            SyncState::Uploading { sent, total } => Status::Uploading {
                percent: if *total == 0 {
                    100
                } else {
                    (sent * 100 / total).min(100) as u8
                },
            },
            SyncState::Offline => {
                if self.entry.has_local_changes() {
                    Status::NotSynced
                } else {
                    Status::Offline
                }
            }
            SyncState::Conflict { .. } => Status::Conflict,
            SyncState::Failed(m) => Status::Failed(m.clone()),
        }
    }
}

/// What the status bar shows for a cloud document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Synced,
    Pending,
    Uploading {
        percent: u8,
    },
    /// Offline with local saves: "Not synced", offer "Sync now".
    NotSynced,
    /// Offline, nothing to upload.
    Offline,
    Conflict,
    ReadOnly(String),
    Failed(String),
}

/// The name of the copy that keeps "my version" next to the original:
/// `Laporan.xlsx` → `Laporan (konflik Budi 2026-11-14).xlsx`.
/// `word` is the localized "conflict"; `who` the user; `date` `YYYY-MM-DD`.
pub fn conflict_copy_name(name: &str, word: &str, who: &str, date: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    let who: String = who
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let mut out = format!("{stem} ({word} {who} {date})");
    if let Some(e) = ext {
        out.push('.');
        out.push_str(e);
    }
    out
}

/// `name` with ` 2`, ` 3`… before the extension, for when a copy name is taken.
pub fn numbered(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => format!("{s} {n}.{e}"),
        _ => format!("{name} {n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::write_atomic;

    fn doc(dir: &std::path::Path) -> DocSync {
        let local = dir.join("Laporan.xlsx");
        write_atomic(&local, b"server v1").unwrap();
        let entry = CacheEntry {
            account: "a".into(),
            remote: "Laporan.xlsx".into(),
            local: local.clone(),
            etag: Some("\"e1\"".into()),
            synced: Stamp::of(&local),
            ..Default::default()
        };
        DocSync::new(entry, LockState::Held(LockToken("t".into())))
    }

    /// The app saving the working copy (different size: the stamp changes
    /// even on coarse-mtime filesystems).
    fn local_save(d: &DocSync, text: &str) {
        std::fs::write(&d.entry.local, text).unwrap();
    }

    fn synced(d: &DocSync, etag: &str) -> UploadOutcome {
        UploadOutcome::Synced {
            server: Some(crate::Entry {
                href: "/Laporan.xlsx".into(),
                name: "Laporan.xlsx".into(),
                is_dir: false,
                size: None,
                etag: Some(etag.into()),
                last_modified: None,
                content_type: None,
            }),
            stamp: Stamp::of(&d.entry.local),
            lock_lost: false,
            sha256: None,
        }
    }

    #[test]
    fn save_uploads_then_synced() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc(dir.path());
        assert_eq!(d.status(), Status::Synced);
        local_save(&d, "edit 1");
        assert_eq!(d.on_local_save(), Action::Upload);
        assert_eq!(d.status(), Status::Pending);
        d.begin_upload(6);
        d.on_progress(3, 6);
        assert_eq!(d.status(), Status::Uploading { percent: 50 });
        let out = synced(&d, "\"e2\"");
        assert_eq!(d.on_upload(out), Action::None);
        assert_eq!(d.status(), Status::Synced);
        assert_eq!(d.entry.etag.as_deref(), Some("\"e2\""));
        assert!(!d.unsynced());
    }

    #[test]
    fn save_during_upload_uploads_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc(dir.path());
        local_save(&d, "edit 1");
        assert_eq!(d.on_local_save(), Action::Upload);
        d.begin_upload(6);
        // The upload read "edit 1"; then the user saves again.
        let uploaded = synced(&d, "\"e2\"");
        local_save(&d, "edit 2, longer");
        assert_eq!(
            d.on_local_save(),
            Action::None,
            "no second upload in flight"
        );
        assert_eq!(d.on_upload(uploaded), Action::Upload);
        assert_eq!(d.status(), Status::Pending);
    }

    #[test]
    fn conflict_from_412_or_recheck() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc(dir.path());
        local_save(&d, "mine");
        d.on_local_save();
        d.begin_upload(4);
        let a = d.on_upload(UploadOutcome::Conflict {
            server_etag: Some("\"theirs\"".into()),
        });
        assert_eq!(a, Action::ShowConflict);
        assert_eq!(d.status(), Status::Conflict);
        // Saving again while in conflict reopens the dialog, never uploads.
        local_save(&d, "mine again");
        assert_eq!(d.on_local_save(), Action::ShowConflict);
        assert_eq!(d.on_reconnect(), Action::None);
        // Resolved by reloading the server version.
        let mut e = d.entry.clone();
        write_atomic(&e.local, b"theirs").unwrap();
        e.etag = Some("\"theirs\"".into());
        e.synced = Stamp::of(&e.local);
        d.on_reloaded(e);
        assert_eq!(d.status(), Status::Synced);
        assert!(!d.unsynced());
    }

    #[test]
    fn offline_keeps_editing_then_sync_now() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc(dir.path());
        local_save(&d, "edit 1");
        assert_eq!(d.on_local_save(), Action::Upload);
        d.begin_upload(6);
        assert_eq!(
            d.on_upload(UploadOutcome::Offline("connection refused".into())),
            Action::None
        );
        assert_eq!(d.status(), Status::NotSynced);
        // More saves while offline don't try to upload.
        local_save(&d, "edit 2 offline");
        assert_eq!(d.on_local_save(), Action::None);
        assert_eq!(d.status(), Status::NotSynced);
        // Server back: upload with the same (ETag) rules.
        assert_eq!(d.on_reconnect(), Action::Upload);
        d.begin_upload(14);
        let out = synced(&d, "\"e3\"");
        assert_eq!(d.on_upload(out), Action::None);
        assert_eq!(d.status(), Status::Synced);
    }

    #[test]
    fn offline_open_without_changes_reconnects_to_synced() {
        let dir = tempfile::tempdir().unwrap();
        let d0 = doc(dir.path());
        let mut d = DocSync::offline(d0.entry.clone());
        assert_eq!(d.status(), Status::Offline);
        assert_eq!(d.on_reconnect(), Action::None);
        assert_eq!(d.status(), Status::Synced);
    }

    #[test]
    fn locked_by_someone_else_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc(dir.path());
        d.lock = LockState::LockedBy("Budi".into());
        assert_eq!(d.status(), Status::ReadOnly("Budi".into()));
        local_save(&d, "local only");
        assert_eq!(d.on_local_save(), Action::NotifyReadOnly);
        assert_eq!(
            d.on_reconnect(),
            Action::None,
            "never uploads while read-only"
        );
        // A lock appearing mid-session turns the document read-only too.
        let mut d2 = doc(dir.path());
        local_save(&d2, "x");
        d2.on_local_save();
        d2.begin_upload(1);
        assert_eq!(
            d2.on_upload(UploadOutcome::LockedBy("Sri".into())),
            Action::NotifyReadOnly
        );
        assert_eq!(d2.status(), Status::ReadOnly("Sri".into()));
    }

    #[test]
    fn busy_after_aborted_upload_retries() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc(dir.path());
        local_save(&d, "edit");
        d.on_local_save();
        d.begin_upload(4);
        assert_eq!(d.on_upload(UploadOutcome::Busy), Action::RetryLater);
        assert_eq!(d.status(), Status::Pending);
        d.begin_upload(4);
        let out = synced(&d, "\"e9\"");
        assert_eq!(d.on_upload(out), Action::None);
    }

    #[test]
    fn failure_is_shown_and_retried_by_sync_now() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc(dir.path());
        local_save(&d, "edit");
        d.on_local_save();
        d.begin_upload(4);
        d.on_upload(UploadOutcome::Failed("HTTP 507".into()));
        assert_eq!(d.status(), Status::Failed("HTTP 507".into()));
        assert_eq!(d.on_reconnect(), Action::Upload);
    }

    #[test]
    fn conflict_names() {
        assert_eq!(
            conflict_copy_name("Laporan.xlsx", "konflik", "Budi", "2026-11-14"),
            "Laporan (konflik Budi 2026-11-14).xlsx"
        );
        assert_eq!(
            conflict_copy_name("Notulen rapat.v2.docx", "conflict", "a/b", "2026-01-02"),
            "Notulen rapat.v2 (conflict a_b 2026-01-02).docx"
        );
        assert_eq!(
            conflict_copy_name("README", "konflik", "Sri", "2026-11-14"),
            "README (konflik Sri 2026-11-14)"
        );
        assert_eq!(
            numbered("A (konflik B 2026-11-14).xlsx", 2),
            "A (konflik B 2026-11-14) 2.xlsx"
        );
    }
}
