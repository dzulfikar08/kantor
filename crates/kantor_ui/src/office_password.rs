//! Password-protected Office files (`.docx`, `.xlsx`, `.pptx` encrypted by
//! Office, `.odt`, `.ods`, `.odp` by LibreOffice; see `kantor_crypt`): the
//! workspace asks for the password before it opens a tab, checks it in the
//! background, and leaves it here for the new tab's view to take
//! ([`take`]), which opens the file with it and keeps it so Save writes the
//! file protected again.
//!
//! A password is kept only in memory: never in settings, the session or
//! recovery copies.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::i18n::{fill, tr};
use crate::tokens::tokens;

/// Passwords checked for a file about to open, each taken once by the view
/// that opens it.
static KNOWN: std::sync::Mutex<Vec<(PathBuf, String)>> = std::sync::Mutex::new(Vec::new());

/// Leave `password` for the next view that opens `path`.
pub fn remember(path: &Path, password: String) {
    if let Ok(mut k) = KNOWN.lock() {
        k.retain(|(p, _)| p != path);
        k.push((path.to_path_buf(), password));
    }
}

/// The password left for `path`, if any (removed).
pub fn take(path: &Path) -> Option<String> {
    let mut k = KNOWN.lock().ok()?;
    let i = k.iter().position(|(p, _)| p == path)?;
    Some(k.remove(i).1)
}

/// Whether a password is waiting for `path`.
pub fn known(path: &Path) -> bool {
    KNOWN.lock().is_ok_and(|k| k.iter().any(|(p, _)| p == path))
}

/// Whether `path` is a password-protected Office or OpenDocument file that
/// needs the user's password (a workbook protected with Excel's default
/// password, "read-only recommended", opens without asking, as in Excel).
pub fn is_protected(path: &Path) -> bool {
    match kantor_crypt::file_protection(path) {
        None => false,
        Some(kantor_crypt::Protection::Odf) => true,
        Some(kantor_crypt::Protection::Ooxml) => std::fs::read(path).map_or(true, |bytes| {
            kantor_crypt::check_password(&bytes, kantor_crypt::DEFAULT_EXCEL_PASSWORD).is_err()
        }),
    }
}

/// The answer: the checked password, `None` if the user cancelled.
type Answer = Box<dyn FnOnce(Option<String>, &mut Window, &mut App)>;
/// Check the typed password (the Open button and Enter).
type Submit = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Default)]
struct Prompt {
    /// What went wrong with the last password (shown under the field).
    error: Option<String>,
    checking: bool,
    done: Option<Answer>,
}

/// Ask for the password of the protected file at `path`, check it, and
/// call `done` with it (`None`: cancelled). Wrong passwords ask again.
pub fn ask(
    path: PathBuf,
    window: &mut Window,
    cx: &mut App,
    done: impl FnOnce(Option<String>, &mut Window, &mut App) + 'static,
) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let title: SharedString = tr("Password", "Kata Sandi").into();
    let detail: SharedString = fill(
        tr(
            "“{}” is protected. Enter the password to open it.",
            "“{}” dilindungi. Masukkan kata sandi untuk membukanya.",
        ),
        &[&name],
    )
    .into();
    let state = Rc::new(RefCell::new(Prompt {
        done: Some(Box::new(done)),
        ..Prompt::default()
    }));
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .masked(true)
            .placeholder(tr("Password", "Kata sandi"))
    });
    let submit: Submit = Rc::new({
        let (state, input, path) = (state.clone(), input.clone(), path.clone());
        move |window, cx| submit(&state, &input, &path, window, cx)
    });
    let sub = window.subscribe(&input, cx, {
        let submit = submit.clone();
        move |_, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                submit(window, cx);
            }
        }
    });
    input.update(cx, |i, cx| i.focus(window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let _keep = &sub;
        let t = tokens(cx);
        let s = state.borrow();
        let cancel = {
            let state = state.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                window.close_dialog(cx);
                if let Some(done) = state.borrow_mut().done.take() {
                    window.defer(cx, move |window, cx| done(None, window, cx));
                }
            }
        };
        let ok = {
            let submit = submit.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| submit(window, cx)
        };
        dialog
            .w(px(420.))
            .overlay_closable(false)
            .title(title.clone())
            .child(
                v_flex()
                    .gap_3()
                    .child(DialogDescription::new().child(detail.clone()))
                    .child(Input::new(&input).cleanable(false))
                    .when_some(s.error.clone(), |d, e| {
                        d.child(div().text_sm().text_color(t.danger).child(e))
                    }),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("office-password-cancel")
                            .label(tr("Cancel", "Batal"))
                            .ghost()
                            .on_click(cancel),
                    )
                    .child(
                        Button::new("office-password-open")
                            .label(tr("Open", "Buka"))
                            .primary()
                            .loading(s.checking)
                            .on_click(ok),
                    ),
            )
    });
}

fn submit(
    state: &Rc<RefCell<Prompt>>,
    input: &Entity<InputState>,
    path: &Path,
    window: &mut Window,
    cx: &mut App,
) {
    let password = input.read(cx).value().to_string();
    if password.is_empty() || state.borrow().checking {
        return;
    }
    state.borrow_mut().checking = true;
    window.refresh();
    let check = cx.background_spawn({
        let (path, password) = (path.to_path_buf(), password.clone());
        async move {
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            kantor_crypt::check_password(&bytes, &password).map_err(|e| match e {
                kantor_crypt::Error::WrongPassword | kantor_crypt::Error::PasswordNeeded => {
                    String::new()
                }
                other => other.to_string(),
            })
        }
    });
    let (state, input) = (state.clone(), input.clone());
    window
        .spawn(cx, async move |cx| {
            let result = check.await;
            let _ = cx.update(|window, cx| {
                state.borrow_mut().checking = false;
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        if let Some(done) = state.borrow_mut().done.take() {
                            done(Some(password), window, cx);
                        }
                    }
                    Err(e) => {
                        state.borrow_mut().error = Some(if e.is_empty() {
                            tr(
                                "The password is incorrect. Try again.",
                                "Kata sandinya salah. Coba lagi.",
                            )
                            .to_owned()
                        } else {
                            fill(
                                tr(
                                    "Kantor can't open this file: {}",
                                    "Kantor tidak bisa membuka berkas ini: {}",
                                ),
                                &[&e],
                            )
                        });
                        input.update(cx, |i, cx| {
                            i.set_value("", window, cx);
                            i.focus(window, cx);
                        });
                        window.refresh();
                    }
                }
            });
        })
        .detach();
}
