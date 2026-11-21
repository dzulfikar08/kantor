//! Writing documents where the App Sandbox may allow only the file itself.
//!
//! Kantor saves through a temporary file next to the document and a
//! rename, so a failed save never leaves half a file. Inside the sandbox
//! ([`crate::SANDBOXED`]) a file the user picked may be writable while its
//! folder isn't, so the temporary file can't be made: then the document is
//! overwritten in place instead ([`no_folder_access`], [`write_in_place`]).
//! Elsewhere nothing changes.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Whether `err` (creating a file next to the document) means the sandbox
/// allows only the document itself.
pub fn no_folder_access(err: &io::Error) -> bool {
    crate::SANDBOXED && err.kind() == io::ErrorKind::PermissionDenied
}

/// Overwrite (or create) `path` with `bytes` and flush it to disk.
pub fn write_in_place(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Write `bytes` to `path` through `<path>.kantor-tmp` and a rename (in
/// place when the sandbox allows only `path`, see the module docs).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".kantor-tmp");
    let tmp = PathBuf::from(tmp);
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return if no_folder_access(&e) {
            write_in_place(path, bytes)
        } else {
            Err(e)
        };
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        write_in_place(&p, b"3").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"3");
    }

    #[test]
    fn falls_back_only_in_the_sandbox() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(no_folder_access(&denied), crate::SANDBOXED);
        assert!(!no_folder_access(&io::Error::from(io::ErrorKind::NotFound)));
    }
}
