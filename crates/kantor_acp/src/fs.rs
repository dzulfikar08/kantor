//! The client's optional `fs/*` capability. Off by default: Kantor's
//! documents are binary files the agent edits through Kantor's MCP tools.
//! A caller that wants it (e.g. for plain-text side files) passes an
//! [`FsHandler`] in [`crate::ClientOptions::fs`].

use std::io;
use std::path::{Path, PathBuf};

/// Answers the agent's `fs/read_text_file` and `fs/write_text_file`.
/// Errors are messages sent back to the agent.
pub trait FsHandler: Send + Sync {
    fn can_read(&self) -> bool {
        true
    }
    fn can_write(&self) -> bool {
        false
    }
    /// `line` is 1-based; `limit` caps the number of lines.
    fn read_text_file(
        &self,
        path: &Path,
        line: Option<u64>,
        limit: Option<u64>,
    ) -> Result<String, String>;
    fn write_text_file(&self, path: &Path, content: &str) -> Result<(), String>;
}

/// Plain files under one folder, read-only unless `write` is set.
#[derive(Debug, Clone)]
pub struct DiskFs {
    root: PathBuf,
    write: bool,
}

impl DiskFs {
    pub fn new(root: impl Into<PathBuf>, write: bool) -> Self {
        DiskFs {
            root: root.into(),
            write,
        }
    }

    /// `path` when it is inside the root (after resolving links; a file
    /// that doesn't exist yet is checked through its folder).
    fn inside(&self, path: &Path) -> Result<PathBuf, String> {
        let deny = || format!("{} is outside the shared folder", path.display());
        if !path.is_absolute() {
            return Err(deny());
        }
        let root = self.root.canonicalize().map_err(|e| e.to_string())?;
        let real = match path.canonicalize() {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let parent = path.parent().ok_or_else(deny)?;
                let name = path.file_name().ok_or_else(deny)?;
                parent.canonicalize().map_err(|e| e.to_string())?.join(name)
            }
            Err(e) => return Err(e.to_string()),
        };
        if real.starts_with(&root) {
            Ok(real)
        } else {
            Err(deny())
        }
    }
}

impl FsHandler for DiskFs {
    fn can_write(&self) -> bool {
        self.write
    }

    fn read_text_file(
        &self,
        path: &Path,
        line: Option<u64>,
        limit: Option<u64>,
    ) -> Result<String, String> {
        let path = self.inside(path)?;
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Ok(slice_lines(&text, line, limit))
    }

    fn write_text_file(&self, path: &Path, content: &str) -> Result<(), String> {
        if !self.write {
            return Err("writing files is off".to_owned());
        }
        let path = self.inside(path)?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }
}

/// Lines `line..line+limit` (1-based) of `text`, line ends kept.
pub(crate) fn slice_lines(text: &str, line: Option<u64>, limit: Option<u64>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_owned();
    }
    let skip = line.unwrap_or(1).saturating_sub(1);
    let take = limit.unwrap_or(u64::MAX);
    text.split_inclusive('\n')
        .skip(usize::try_from(skip).unwrap_or(usize::MAX))
        .take(usize::try_from(take).unwrap_or(usize::MAX))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_lines() {
        let t = "a\nb\nc\n";
        assert_eq!(slice_lines(t, None, None), t);
        assert_eq!(slice_lines(t, Some(2), None), "b\nc\n");
        assert_eq!(slice_lines(t, Some(2), Some(1)), "b\n");
        assert_eq!(slice_lines(t, None, Some(1)), "a\n");
        assert_eq!(slice_lines(t, Some(9), None), "");
    }

    #[test]
    fn stays_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("a.txt"), "hi\n").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "no").unwrap();

        let ro = DiskFs::new(&inner, false);
        assert_eq!(
            ro.read_text_file(&inner.join("a.txt"), None, None).unwrap(),
            "hi\n"
        );
        assert!(
            ro.read_text_file(&dir.path().join("secret.txt"), None, None)
                .is_err()
        );
        assert!(
            ro.read_text_file(&inner.join("..").join("secret.txt"), None, None)
                .is_err()
        );
        assert!(ro.read_text_file(Path::new("a.txt"), None, None).is_err());
        assert!(ro.write_text_file(&inner.join("b.txt"), "x").is_err());

        let rw = DiskFs::new(&inner, true);
        rw.write_text_file(&inner.join("b.txt"), "x").unwrap();
        assert_eq!(std::fs::read_to_string(inner.join("b.txt")).unwrap(), "x");
        assert!(rw.write_text_file(&dir.path().join("c.txt"), "x").is_err());
    }
}
