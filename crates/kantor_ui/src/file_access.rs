//! "Grant File Access" for the sandboxed Mac App Store build (see
//! `kantor_sandbox`): when Kantor knows a file's path but macOS hasn't let
//! it use the file in this run (last session's tabs, recent files, a
//! linked data source or picture, Batch folders), it asks the user to pick
//! it in an open panel that starts on its folder, like Word for Mac. Outside
//! the sandbox nothing is asked.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::i18n::{fill, tr};

/// What Kantor needs to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The file itself (to open or save it).
    File,
    /// A folder (to read or create files in it).
    Folder,
}

/// Make `path` usable, then call `done` with the path to use: `path`
/// itself when Kantor may already use it, else what the user picked after
/// being asked (`None`: they cancelled). Outside the sandbox `done` runs at
/// once with `path`.
pub fn with_access(
    path: PathBuf,
    need: Need,
    window: &mut Window,
    cx: &mut App,
    done: impl FnOnce(Option<PathBuf>, &mut Window, &mut App) + 'static,
) {
    if kantor_sandbox::access(&path) {
        done(Some(path), window, cx);
        return;
    }
    ask(path, need, window, cx, done);
}

/// Make every one of `needs` usable, asking for each that isn't, one after
/// another; `done(true)` once all are (at once outside the sandbox),
/// `done(false)` when the user cancelled or picked something else.
pub fn with_access_all(
    needs: Vec<(PathBuf, Need)>,
    window: &mut Window,
    cx: &mut App,
    done: impl FnOnce(bool, &mut Window, &mut App) + 'static,
) {
    let mut missing: Vec<(PathBuf, Need)> = needs
        .into_iter()
        .filter(|(p, _)| !kantor_sandbox::access(p))
        .collect();
    missing.reverse();
    next_missing(missing, window, cx, Box::new(done));
}

fn next_missing(
    mut missing: Vec<(PathBuf, Need)>,
    window: &mut Window,
    cx: &mut App,
    done: Box<dyn FnOnce(bool, &mut Window, &mut App)>,
) {
    let Some((path, need)) = missing.pop() else {
        done(true, window, cx);
        return;
    };
    // An earlier answer (a folder above it) may have covered this one.
    if kantor_sandbox::access(&path) {
        next_missing(missing, window, cx, done);
        return;
    }
    ask(path.clone(), need, window, cx, move |_, window, cx| {
        if kantor_sandbox::access(&path) {
            next_missing(missing, window, cx, done);
        } else {
            done(false, window, cx);
        }
    });
}

/// [`with_access`] for the folder a new file at `path` goes in (Save next
/// to the original, a signed or converted copy); `done` gets `path` back
/// when Kantor may write there.
pub fn with_folder_of(
    path: PathBuf,
    window: &mut Window,
    cx: &mut App,
    done: impl FnOnce(Option<PathBuf>, &mut Window, &mut App) + 'static,
) {
    let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) else {
        done(Some(path), window, cx);
        return;
    };
    let dir = dir.to_path_buf();
    with_access(dir, Need::Folder, window, cx, move |picked, window, cx| {
        let target = picked.map(|d| match path.file_name() {
            Some(name) => d.join(name),
            None => path,
        });
        done(target, window, cx);
    });
}

/// The dialog's title and text for `path`.
pub fn wording(path: &Path, need: Need) -> (String, String) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let title = match need {
        Need::File => fill(
            tr(
                "Kantor needs your permission to open “{}”",
                "Kantor perlu izin Anda untuk membuka “{}”",
            ),
            &[&name],
        ),
        Need::Folder => fill(
            tr(
                "Kantor needs your permission to use the folder “{}”",
                "Kantor perlu izin Anda untuk memakai folder “{}”",
            ),
            &[&name],
        ),
    };
    let detail = tr(
        "macOS lets apps from the App Store use only the files and folders you choose. Click Grant Access, then choose it in the next window.",
        "macOS hanya mengizinkan aplikasi dari App Store memakai berkas dan folder yang Anda pilih. Klik Beri Akses, lalu pilih di jendela berikutnya.",
    )
    .to_owned();
    (title, detail)
}

fn ask(
    path: PathBuf,
    need: Need,
    window: &mut Window,
    cx: &mut App,
    done: impl FnOnce(Option<PathBuf>, &mut Window, &mut App) + 'static,
) {
    let (title, detail) = wording(&path, need);
    let (title, detail): (SharedString, SharedString) = (title.into(), detail.into());
    // The dialog's buttons are `Fn`; the answer runs once.
    let done: Rc<std::cell::RefCell<Option<Box<dyn FnOnce(Option<PathBuf>, &mut Window, &mut App)>>>> =
        Rc::new(std::cell::RefCell::new(Some(Box::new(done))));
    window.open_dialog(cx, move |dialog, _, _| {
        let cancel = {
            let done = done.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                window.close_dialog(cx);
                if let Some(done) = done.borrow_mut().take() {
                    window.defer(cx, move |window, cx| done(None, window, cx));
                }
            }
        };
        let grant = {
            let done = done.clone();
            let path = path.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                window.close_dialog(cx);
                if let Some(done) = done.borrow_mut().take() {
                    pick(path.clone(), need, window, cx, done);
                }
            }
        };
        dialog
            .w(px(480.))
            .overlay_closable(false)
            .close_button(false)
            .title(title.clone())
            .child(v_flex().child(DialogDescription::new().child(detail.clone())))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("grant-cancel")
                            .label(tr("Cancel", "Batal"))
                            .ghost()
                            .on_click(cancel),
                    )
                    .child(
                        Button::new("grant-access")
                            .label(tr("Grant Access…", "Beri Akses…"))
                            .primary()
                            .on_click(grant),
                    ),
            )
    });
}

/// The open panel, on `path`'s folder (a folder: on the folder itself, so
/// Grant Access picks it).
fn pick(
    path: PathBuf,
    need: Need,
    window: &mut Window,
    cx: &mut App,
    done: Box<dyn FnOnce(Option<PathBuf>, &mut Window, &mut App)>,
) {
    let start = match need {
        Need::Folder if path.is_dir() => Some(path.clone()),
        _ => kantor_sandbox::grant_panel_dir(&path),
    };
    let message = wording(&path, need).0;
    next_open_panel(start, message);
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: need == Need::File,
        directories: need == Need::Folder,
        multiple: false,
        prompt: Some(tr("Grant Access", "Beri Akses").into()),
    });
    window
        .spawn(cx, async move |cx| {
            let picked = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            if let Some(p) = &picked {
                kantor_sandbox::remember(p);
            }
            let _ = cx.update(|window, cx| done(picked, window, cx));
        })
        .detach();
}

#[cfg(target_os = "macos")]
fn next_open_panel(directory: Option<PathBuf>, message: String) {
    gpui_macos::set_next_open_panel(directory, Some(message));
}

#[cfg(not(target_os = "macos"))]
fn next_open_panel(_: Option<PathBuf>, _: String) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn wording_names_the_file_or_folder() {
        let (title, detail) = wording(Path::new("/Users/ana/Laporan/Q3.xlsx"), Need::File);
        assert!(title.contains("“Q3.xlsx”"), "{title}");
        assert!(detail.contains("App Store"));
        let (title, _) = wording(Path::new("/Users/ana/Laporan"), Need::Folder);
        assert!(title.contains("folder “Laporan”"), "{title}");
    }
}
