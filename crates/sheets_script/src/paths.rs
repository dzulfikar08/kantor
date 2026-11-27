//! Files a macro reads or writes (`Shapes.AddPicture`, `ExportAsFixedFormat`,
//! `Chart.Export`): inside the workbook's folder (or a folder below it) they
//! are allowed; anything else goes through the same check as
//! `Workbooks.Open` and `SaveAs` ([`crate::books::check_path`]): paths given
//! with `--allow-path` or allowed before in the engine's life, else
//! [`crate::MacroUi::allow_path`], which the app answers by asking the user
//! and the command line (`HeadlessUi`) refuses. A workbook that was never
//! saved has no folder, so every path is checked.

use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use kantor_script::ScriptError;

use crate::session::Session;
use crate::values::err1004;

/// `path` made absolute (relative to the workbook's folder) with `.` and
/// `..` resolved lexically.
fn absolute(folder: &str, path: &str) -> PathBuf {
    let p = Path::new(path);
    let joined = if p.is_absolute() || folder.is_empty() {
        p.to_path_buf()
    } else {
        Path::new(folder).join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `path` is inside `folder` (both made canonical where they
/// exist, so a symbolic link can't lead out).
fn inside(folder: &str, path: &Path) -> bool {
    if folder.is_empty() {
        return false;
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = canon(Path::new(folder));
    // The file may not exist yet: check its folder.
    let target = if path.exists() {
        canon(path)
    } else {
        match (path.parent(), path.file_name()) {
            (Some(dir), Some(name)) => canon(dir).join(name),
            _ => return false,
        }
    };
    target.starts_with(&root)
}

/// The file a macro may use at `path`, or error 1004 when it may not.
pub fn checked(s: &Rc<Session>, path: &str, write: bool) -> Result<PathBuf, ScriptError> {
    let folder = s.folder.borrow().clone();
    let abs = absolute(&folder, path.trim());
    if abs.as_os_str().is_empty() {
        return Err(err1004("A file name is needed"));
    }
    if inside(&folder, &abs) {
        return Ok(abs);
    }
    crate::books::check_path(s, &abs, write)?;
    Ok(abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_resolve_in_the_folder() {
        let dir = std::env::temp_dir();
        let folder = dir.display().to_string();
        let p = absolute(&folder, "out.pdf");
        assert_eq!(p, dir.join("out.pdf"));
        assert!(inside(&folder, &p));
        let up = absolute(&folder, &format!("..{}x.pdf", std::path::MAIN_SEPARATOR));
        assert!(!inside(&folder, &up));
        assert!(!inside("", &p));
    }
}
