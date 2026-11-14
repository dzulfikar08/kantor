//! Asking before Save writes over a file that changed on disk.
//!
//! A document restored from a recovery copy still points at its original
//! file. If someone (another app, a sync client, another Kantor) changed that
//! file since Kantor last read or wrote it, Save asks first instead of
//! overwriting their changes.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::recovery::FileStamp;

use crate::i18n::{t, tf};

/// The file Save would overwrite, as Kantor last saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskGuard {
    path: PathBuf,
    stamp: Option<FileStamp>,
}

impl DiskGuard {
    pub fn new(path: PathBuf, stamp: Option<FileStamp>) -> Self {
        Self { path, stamp }
    }

    /// Whether saving to `target` would overwrite changes made elsewhere
    /// (the file changed, appeared or went away since).
    pub fn changed(&self, target: &Path) -> bool {
        target == self.path && FileStamp::of(&self.path) != self.stamp
    }
}

/// What the user chose in [`confirm_overwrite`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overwrite {
    /// Save over it anyway.
    Replace,
    /// Save to another file.
    SaveAs,
}

/// Ask whether to overwrite `name`, changed on disk; `answer` runs after the
/// dialog closes (Cancel does nothing).
pub fn confirm_overwrite(
    name: &str,
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(Overwrite, &mut Window, &mut App) + 'static,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let title: SharedString = tf("“{}” changed since Kantor last saved it", &[&name]).into();
    let detail: SharedString = t("Another app or person may have edited it. Replace it with your version, or save yours as a new file to keep both.")
        .into();
    let answer = Rc::new(answer);
    window.open_dialog(cx, move |dialog, _, _| {
        let reply = |choice: Overwrite| {
            let answer = answer.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                window.close_dialog(cx);
                let answer = answer.clone();
                window.defer(cx, move |window, cx| answer(choice, window, cx));
            }
        };
        let save_as = reply(Overwrite::SaveAs);
        dialog
            .w(px(460.))
            .overlay_closable(false)
            .close_button(false)
            .title(title.clone())
            .child(v_flex().child(DialogDescription::new().child(detail.clone())))
            .on_ok({
                let answer = answer.clone();
                move |_, window, cx| {
                    let answer = answer.clone();
                    window.defer(cx, move |window, cx| answer(Overwrite::SaveAs, window, cx));
                    true
                }
            })
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("overwrite-cancel")
                            .label(t("Cancel"))
                            .ghost()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("overwrite-replace")
                            .label(t("Replace"))
                            .danger()
                            .on_click(reply(Overwrite::Replace)),
                    )
                    .child(
                        Button::new("overwrite-save-as")
                            .label(t("Save As…"))
                            .primary()
                            .on_click(save_as),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn notices_changes_to_the_guarded_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.xlsx");
        std::fs::write(&path, b"one").unwrap();
        let guard = DiskGuard::new(path.clone(), FileStamp::of(&path));
        assert!(!guard.changed(&path));
        assert!(!guard.changed(&dir.path().join("other.xlsx")));
        std::fs::write(&path, b"changed").unwrap();
        assert!(guard.changed(&path));
        std::fs::remove_file(&path).unwrap();
        assert!(guard.changed(&path), "deleted counts as changed");
        // Never saved before: a file appearing there counts too.
        let fresh = DiskGuard::new(path.clone(), None);
        assert!(!fresh.changed(&path));
        std::fs::write(&path, b"new").unwrap();
        assert!(fresh.changed(&path));
    }
}
