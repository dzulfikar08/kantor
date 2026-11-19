//! Opening files handed over by the system: command-line paths, Open With
//! and double-clicks (macOS sends them while Kantor runs, see
//! `crate::open_files`), and files dragged onto the window.
//!
//! Each supported file opens in a tab (or comes to the front if it is open
//! already). A file Kantor can't open gets Excel's message in a dialog.
//! PNG and JPEG files dropped on a Docs document or a presentation are
//! inserted as pictures, as Word and PowerPoint do.

use std::path::{Path, PathBuf};

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, tf};
use kantor_ui::tokens;

use super::doc::DocView;
use super::{AppKind, Workspace};
use crate::open_files::is_picture;

/// How dropped files are handled.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DropPlan {
    /// Inserted into the focused document as pictures.
    pub pictures: Vec<PathBuf>,
    /// Opened as documents (or refused, see [`Workspace::open_paths`]).
    pub open: Vec<PathBuf>,
}

/// Pictures go into a focused Docs document or presentation when that is
/// where they were dropped; everything else is opened.
pub(crate) fn plan_drop(paths: &[PathBuf], takes_pictures: bool) -> DropPlan {
    let (pictures, open) = paths
        .iter()
        .cloned()
        .partition(|p| takes_pictures && is_picture(p));
    DropPlan { pictures, open }
}

/// Which app opens `path` (`app` when the command line names one), or
/// `None` when Kantor can't open it.
pub(crate) fn app_for(app: Option<AppKind>, path: &Path) -> Option<AppKind> {
    if path.is_dir() {
        return None;
    }
    app.or_else(|| AppKind::for_path(path))
}

/// Excel's wording for a file it can't open.
pub(crate) fn cant_open_message(paths: &[PathBuf]) -> String {
    let name = |p: &PathBuf| {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    match paths {
        [one] => tf(
            "Kantor can't open the file “{}” because the file format or file extension is not valid. Verify that the file has not been corrupted and that the file extension matches the format of the file.",
            &[&name(one)],
        ),
        many => tf(
            "Kantor can't open these files because their file format or file extension is not valid: {}. Verify that the files have not been corrupted and that their file extensions match their formats.",
            &[&many
                .iter()
                .map(|p| format!("“{}”", name(p)))
                .collect::<Vec<_>>()
                .join(", ")],
        ),
    }
}

impl Workspace {
    /// Open each of `paths` in a tab (in the app `app` when given, else the
    /// one its extension calls for); a dialog names the ones Kantor can't
    /// open.
    pub fn open_paths(
        &mut self,
        app: Option<AppKind>,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let refused = self.launch_paths(app, paths, window, cx);
        if !refused.is_empty() {
            self.cant_open(&refused, window, cx);
        }
    }

    /// Open the supported files among `paths`; returns the others.
    pub(super) fn launch_paths(
        &mut self,
        app: Option<AppKind>,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<PathBuf> {
        let mut refused = Vec::new();
        for path in paths {
            match app_for(app, &path) {
                Some(kind) => self.launch(kind, Some(path), window, cx),
                None => refused.push(path),
            }
        }
        refused
    }

    /// Excel's "can't open" message for `paths`, in a dialog.
    pub(super) fn cant_open(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut App) {
        let detail: SharedString = cant_open_message(paths).into();
        log::warn!("{detail}");
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(440.))
                .title("Kantor")
                .child(v_flex().child(DialogDescription::new().child(detail.clone())))
                .footer(
                    DialogFooter::new().child(
                        Button::new("cant-open-ok")
                            .label(t("OK"))
                            .primary()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
        });
    }

    /// Files dropped on the window: pictures into the focused Docs document
    /// or presentation, the rest opened in tabs.
    pub(super) fn drop_files(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.focused_view();
        let takes_pictures = matches!(view, Some(DocView::Docs(_) | DocView::Slides(_)));
        let plan = plan_drop(paths.paths(), takes_pictures);
        for picture in &plan.pictures {
            match &view {
                Some(DocView::Docs(v)) => {
                    v.update(cx, |v, cx| v.insert_image_file(picture, window, cx))
                }
                Some(DocView::Slides(v)) => {
                    v.update(cx, |v, cx| v.insert_image_file(picture, window, cx))
                }
                _ => {}
            }
        }
        window.activate_window();
        if !plan.open.is_empty() {
            self.open_paths(None, plan.open, window, cx);
        }
    }

    /// A layer over the whole window that takes dropped files and shows
    /// where they go while they are dragged over it.
    pub(super) fn render_drop_target(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tk = tokens(cx);
        let (accent, soft) = (tk.accent, tk.accent_soft);
        div()
            .id("file-drop")
            .absolute()
            .inset_0()
            .drag_over::<ExternalPaths>(move |s, _, _, _| {
                s.bg(soft.opacity(0.35)).border_2().border_color(accent)
            })
            .on_drop(
                cx.listener(|ws, paths: &ExternalPaths, window, cx| {
                    ws.drop_files(paths, window, cx)
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[std::prelude::v1::test]
    fn dropped_pictures_go_into_docs_and_slides_only() {
        let dropped = paths(&[
            "a.xlsx",
            "photo.JPG",
            "b.docx",
            "logo.png",
            "notes.txt",
            "c.pdf",
        ]);
        let plan = plan_drop(&dropped, true);
        assert_eq!(plan.pictures, paths(&["photo.JPG", "logo.png"]));
        // A PDF dropped on a document opens in the viewer.
        assert_eq!(
            plan.open,
            paths(&["a.xlsx", "b.docx", "notes.txt", "c.pdf"])
        );
        // Sheets or Home: pictures are opened, which Kantor refuses.
        let plan = plan_drop(&dropped, false);
        assert!(plan.pictures.is_empty());
        assert_eq!(plan.open, dropped);
    }

    #[std::prelude::v1::test]
    fn apps_for_paths() {
        assert_eq!(app_for(None, Path::new("a.XLSX")), Some(AppKind::Sheets));
        assert_eq!(app_for(None, Path::new("a.tsv")), Some(AppKind::Sheets));
        assert_eq!(app_for(None, Path::new("a.dot")), Some(AppKind::Docs));
        assert_eq!(app_for(None, Path::new("a.pps")), Some(AppKind::Slides));
        assert_eq!(app_for(None, Path::new("Report.PDF")), Some(AppKind::Pdf));
        assert_eq!(app_for(None, Path::new("a.zip")), None);
        assert_eq!(app_for(None, Path::new("README")), None);
        // `kantor --sheets data.txt`: the flag decides.
        assert_eq!(
            app_for(Some(AppKind::Sheets), Path::new("data.txt")),
            Some(AppKind::Sheets)
        );
        // A folder never opens.
        let dir = std::env::temp_dir();
        assert_eq!(app_for(Some(AppKind::Sheets), &dir), None);
    }

    #[std::prelude::v1::test]
    fn cant_open_wording() {
        let one = cant_open_message(&paths(&["/home/a/archive.zip"]));
        assert!(one.contains("“archive.zip”"), "{one}");
        assert!(one.contains("file format or file extension is not valid"));
        let many = cant_open_message(&paths(&["a.zip", "b.mp4"]));
        assert!(many.contains("“a.zip”, “b.mp4”"), "{many}");
    }
}
